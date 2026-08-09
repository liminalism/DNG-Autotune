//! Output metadata: EXIF copied from the RAW, and an embedded sRGB ICC profile.
//!
//! # Why this exists
//!
//! Until 0.1.16 the rendered files carried no metadata at all. A photo library
//! given a folder of them sorts every frame by file modification time, shows no
//! camera or lens, and cannot place anything on a map — so the output could not
//! replace the camera's own JPEG no matter how good the pixels were. This module
//! copies the capture metadata across and declares the colour space the renderer
//! actually produced.
//!
//! # What is copied, and what is deliberately not
//!
//! Copied from the source file: `DateTimeOriginal` / `CreateDate` / `ModifyDate`
//! and their sub-second and time-zone companions, `Make`, `Model`, `LensMake`,
//! `LensModel`, `LensSpecification`, `ExposureTime`, `FNumber`, `ISOSpeedRatings`,
//! `FocalLength`, the metering/flash/exposure-program group, `Artist`,
//! `Copyright`, and the whole GPS IFD when the file has one.
//!
//! Not copied:
//!
//! * **MakerNotes.** They are vendor-private blobs full of absolute file offsets;
//!   moving them into a different file silently corrupts them. Rawler does not
//!   expose them for writing either.
//! * **The source's `Orientation`.** See [`ORIENTATION_NORMAL`] — this is the one
//!   tag that must *not* be copied, and getting it wrong is invisible until an
//!   entire archive is sideways.
//! * **The source's `ColorSpace`.** The renderer always emits sRGB-encoded data,
//!   so the output declares sRGB regardless of what the RAW claimed.
//! * **Thumbnails (IFD1).** Nothing here generates one, so the EXIF block has a
//!   single IFD and `next_ifd` = 0.
//!
//! # Per-format behaviour
//!
//! | Format | EXIF | ICC |
//! |--------|------|-----|
//! | JPEG   | `APP1` `Exif\0\0` segment | `APP2` `ICC_PROFILE` segment |
//! | TIFF   | root IFD + `ExifIFD` (34665) + `GPSInfo` (34853) | `InterColorProfile` (34675) |
//! | PNG    | `eXIf` chunk | `iCCP` chunk |
//!
//! PNG gets the same payload as the other two, but the `eXIf` chunk is a 2017
//! addition to the PNG spec and reader support is thin: Windows Explorer and the
//! Photos app ignore it, while Pillow, exiftool, and macOS read it. PNG output is
//! therefore correct but not as widely *useful* as JPEG or TIFF; the `iCCP`
//! profile, which is old and universally supported, still makes the colour
//! unambiguous. This is a limitation of PNG readers, not a gap here.
//!
//! No `sRGB` chunk is written for PNG because `image`'s encoder does not expose
//! it; `iCCP` is the stronger signal and both would have to agree anyway.
//!
//! # Determinism
//!
//! Nothing here reads the clock. The only timestamps written come from the source
//! file, and the ICC profile's creation date is a fixed constant. IFD entries are
//! held in a `BTreeMap` by [`rawler`]'s `DirectoryWriter`, so they are emitted in
//! ascending tag order, and the ICC tag table is emitted in a fixed
//! signature-sorted order. Same input, same bytes, on any thread count.

use crate::tone::Rgb16Image;
use crate::types::{JpegSettings, JpegSubsampling};
use anyhow::{Context, Result};
use jpeg_encoder::{ColorType as JpegColorType, Encoder as JpegEncoder, SamplingFactor};
use rawler::RawLoader;
use rawler::decoders::{RawDecodeParams, RawMetadata};
use rawler::exif::Exif;
use rawler::formats::tiff::reader::TiffReader;
use rawler::formats::tiff::writer::{DirectoryWriter, TiffWriter};
use rawler::formats::tiff::{GenericTiffReader, IFD, Rational};
use rawler::rawsource::RawSource;
use rawler::tags::{ExifTag, TiffCommonTag, TiffTag};
use std::fs::File;
use std::io::{BufReader, BufWriter, Cursor, Seek, Write};
use std::path::Path;
use std::sync::OnceLock;

/// EXIF `Orientation` written into every output file: 1, "normal".
///
/// **This is not the source's orientation, and must never be.** The pipeline
/// already rotates the pixels — `pipeline::process_job_inner` calls
/// `orientation::apply_orientation(linear, source_orientation)`, which physically
/// flips and transposes the buffer, so what reaches the encoder is upright and
/// its width/height are already swapped for a portrait frame. Copying the RAW's
/// `Orientation` (6 or 8 for a portrait Sony frame) would tell every viewer to
/// rotate an already-rotated image, and the whole archive would be sideways with
/// nothing in the pixels to show it. So the tag is written explicitly as 1 rather
/// than omitted, because an absent tag is merely "unknown" while 1 is a positive
/// statement that no further rotation is wanted.
pub const ORIENTATION_NORMAL: u16 = 1;

/// EXIF `ColorSpace` = 1, sRGB.
///
/// `tone::render` ends in `tone::srgb_encode`, so the output transfer function is
/// the sRGB one and the primaries are Rawler's sRGB calibration target. Whatever
/// the source claimed (Sony bodies write 1 or 65535/Uncalibrated depending on the
/// menu) is irrelevant to what this program produced.
const COLOR_SPACE_SRGB: u16 = 1;

/// TIFF `PlanarConfiguration` = 1, chunky (interleaved RGBRGB...).
const PLANAR_CHUNKY: u16 = 1;
/// TIFF `PhotometricInterpretation` = 2, RGB.
const PHOTOMETRIC_RGB: u16 = 2;
/// TIFF `Compression` = 1, none.
const COMPRESSION_NONE: u16 = 1;
/// TIFF `SampleFormat` = 1, unsigned integer.
const SAMPLE_FORMAT_UINT: u16 = 1;
/// TIFF `ResolutionUnit` = 2, inches.
const RESOLUTION_UNIT_INCH: u16 = 2;
/// Nominal resolution written so viewers have something to print by. Meaningless
/// for a screen image, but a missing `XResolution` upsets some readers.
const NOMINAL_DPI: u32 = 72;

/// Rows per TIFF strip.
///
/// Strips exist so the sample buffer can be converted to bytes a slice at a time
/// instead of allocating a second full-size copy: a 24-megapixel RGB16 frame is
/// 145 MB, and this keeps the extra allocation at about 1 MB.
const TIFF_STRIP_ROWS: usize = 64;

/// Largest EXIF payload a JPEG `APP1` segment can hold.
///
/// The segment length field is two bytes and counts itself, so the payload is
/// capped at 65533, of which the `Exif\0\0` header takes six.
const MAX_JPEG_EXIF_PAYLOAD: usize = 65533 - 6;

/// `Software`, as written to both the TIFF root IFD and the EXIF block.
pub const SOFTWARE: &str = concat!("raw-autotune ", env!("CARGO_PKG_VERSION"));

/// Shared `RawLoader`.
///
/// Constructing one parses the bundled camera database, which is far too slow to
/// repeat per file — the same reasoning as `shotinfo::loader`.
fn loader() -> &'static RawLoader {
    static LOADER: OnceLock<RawLoader> = OnceLock::new();
    LOADER.get_or_init(RawLoader::new)
}

/// Strip a string down to something an EXIF ASCII field can hold.
///
/// Returns `None` for anything that would end up empty. Control characters are
/// dropped because an interior NUL would make `TiffAscii`'s `CString::new`
/// panic, and non-ASCII is dropped because the EXIF ASCII type is 7-bit.
fn ascii(value: &str) -> Option<String> {
    let cleaned: String = value
        .chars()
        .filter(|character| character.is_ascii() && !character.is_control())
        .collect();
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Apply [`ascii`] to every free-text field this module might write.
///
/// Rawler's reader truncates ASCII values at the first NUL so interior NULs
/// cannot normally reach us, but a corrupt file should not be able to panic a
/// batch on a `CString::new` unwrap deep inside the TIFF writer.
fn sanitize(exif: &mut Exif) {
    let fields = [
        &mut exif.copyright,
        &mut exif.artist,
        &mut exif.date_time_original,
        &mut exif.create_date,
        &mut exif.modify_date,
        &mut exif.offset_time,
        &mut exif.offset_time_original,
        &mut exif.offset_time_digitized,
        &mut exif.sub_sec_time,
        &mut exif.sub_sec_time_original,
        &mut exif.sub_sec_time_digitized,
        &mut exif.owner_name,
        &mut exif.serial_number,
        &mut exif.lens_serial_number,
        &mut exif.lens_make,
        &mut exif.lens_model,
        &mut exif.user_comment,
    ];
    for field in fields {
        *field = field.as_deref().and_then(ascii);
    }

    // The GPS block's `*_ref` fields are one-character strings ("N", "E", "K"),
    // and its numeric fields are rationals that need no cleaning.
    if let Some(gps) = &mut exif.gps {
        for field in [
            &mut gps.gps_latitude_ref,
            &mut gps.gps_longitude_ref,
            &mut gps.gps_satellites,
            &mut gps.gps_status,
            &mut gps.gps_measure_mode,
            &mut gps.gps_speed_ref,
            &mut gps.gps_track_ref,
            &mut gps.gps_img_direction_ref,
            &mut gps.gps_map_datum,
            &mut gps.gps_dest_latitude_ref,
            &mut gps.gps_dest_longitude_ref,
            &mut gps.gps_dest_bearing_ref,
            &mut gps.gps_dest_distance_ref,
            &mut gps.gps_date_stamp,
        ] {
            *field = field.as_deref().and_then(ascii);
        }
    }
}

/// Read an ASCII entry from an IFD, sanitized.
fn string_entry<T: TiffTag>(ifd: &IFD, tag: T) -> Option<String> {
    ifd.get_entry(tag)
        .and_then(|entry| entry.value.as_string())
        .map(String::as_str)
        .and_then(ascii)
}

/// Capture metadata read from one source RAW, ready to be written to an output.
///
/// Built by [`SourceMetadata::read`], which never fails: a file whose metadata
/// cannot be parsed still gets `Software`, `Orientation`, `ColorSpace` and the
/// output dimensions, which is strictly better than the nothing that was written
/// before.
#[derive(Debug, Clone, Default)]
pub struct SourceMetadata {
    /// Rawler's parse of the source EXIF. Holds the GPS IFD too.
    raw: RawMetadata,
    /// `Make` exactly as the file spells it, preferred over Rawler's cleaned-up
    /// name so the output groups with the camera's own JPEGs in a library
    /// ("SONY", not "Sony").
    make: Option<String>,
    /// `Model` exactly as the file spells it.
    model: Option<String>,
}

/// Minimal recorded identity needed for deterministic exact lens matching.
#[derive(Debug, Clone)]
pub(crate) struct LensProfileMetadata {
    pub camera_make: String,
    pub camera_model: String,
    pub lens_make: Option<String>,
    pub lens_model: Option<String>,
    pub focal_length: Option<Rational>,
}

pub(crate) fn lens_profile_metadata(path: &Path) -> LensProfileMetadata {
    let metadata = SourceMetadata::read(path);
    let resolved_lens = metadata.raw.lens.as_ref();
    LensProfileMetadata {
        camera_make: metadata.raw.make,
        camera_model: metadata.raw.model,
        lens_make: resolved_lens
            .map(|lens| lens.lens_make.clone())
            .filter(|value| !value.is_empty())
            .or(metadata.raw.exif.lens_make),
        lens_model: resolved_lens
            .map(|lens| lens.lens_model.clone())
            .filter(|value| !value.is_empty())
            .or(metadata.raw.exif.lens_model),
        focal_length: metadata.raw.exif.focal_length,
    }
}

impl SourceMetadata {
    /// Read metadata from `path`. Best effort; never fails.
    pub fn read(path: &Path) -> Self {
        let mut metadata = Self {
            raw: read_raw_metadata(path).unwrap_or_default(),
            make: None,
            model: None,
        };
        sanitize(&mut metadata.raw.exif);

        // Rawler's `Exif` has no parse arm for LensMake/LensModel: it fills them
        // only from its lens database, which resolves for Sony ARW but leaves DNG
        // sources blank even when the file states the lens outright. Read the two
        // tags directly and use them as a fallback, so Samsung and ProShot DNGs
        // carry a lens too.
        if let Ok(file) = File::open(path) {
            let mut reader = BufReader::new(file);
            if let Ok(tiff) = GenericTiffReader::new(&mut reader, 0, 0, None, &[]) {
                let root = tiff.root_ifd();
                metadata.make = string_entry(root, TiffCommonTag::Make);
                metadata.model = string_entry(root, TiffCommonTag::Model);

                if let Some(exif_ifd) = root.get_sub_ifd(ExifTag::ExifOffset) {
                    if metadata.raw.exif.lens_model.is_none() {
                        metadata.raw.exif.lens_model = string_entry(exif_ifd, ExifTag::LensModel);
                    }
                    if metadata.raw.exif.lens_make.is_none() {
                        metadata.raw.exif.lens_make = string_entry(exif_ifd, ExifTag::LensMake);
                    }
                }
            }
        }

        metadata
    }

    /// Build from an already-parsed [`Exif`]. Used by the tests, which need to
    /// control the input exactly.
    pub fn from_exif(exif: Exif) -> Self {
        let mut raw = RawMetadata {
            exif,
            ..Default::default()
        };
        sanitize(&mut raw.exif);
        Self {
            raw,
            make: None,
            model: None,
        }
    }

    /// Override `Make`/`Model` with the strings the caller prefers.
    ///
    /// The pipeline has `RawImage::clean_make`/`clean_model` in hand already, and
    /// they are a reasonable fallback for a source whose root IFD could not be
    /// read (a non-TIFF container, for instance).
    pub fn with_camera(mut self, make: &str, model: &str) -> Self {
        if self.make.is_none() {
            self.make = ascii(make);
        }
        if self.model.is_none() {
            self.model = ascii(model);
        }
        self
    }

    /// The source file's own `Orientation`, for tests and diagnostics.
    ///
    /// Never written to an output; see [`ORIENTATION_NORMAL`].
    pub fn source_orientation(&self) -> Option<u16> {
        self.raw.exif.orientation
    }

    /// `DateTimeOriginal` as the source recorded it, if it did.
    pub fn date_time_original(&self) -> Option<&str> {
        self.raw.exif.date_time_original.as_deref()
    }

    /// True when nothing identifying could be read, i.e. the output will carry
    /// only what this program knows about itself.
    pub fn is_empty(&self) -> bool {
        self.make.is_none()
            && self.model.is_none()
            && self.raw.exif == Exif::default()
            && self.raw.make.is_empty()
            && self.raw.model.is_empty()
    }

    fn make_string(&self) -> Option<String> {
        self.make.clone().or_else(|| ascii(&self.raw.make))
    }

    fn model_string(&self) -> Option<String> {
        self.model.clone().or_else(|| ascii(&self.raw.model))
    }

    /// Populate a root IFD and an EXIF IFD with everything but the image layout.
    ///
    /// Shared by the standalone EXIF payload (JPEG/PNG) and the TIFF writer, so
    /// all three formats necessarily agree on tag values.
    fn fill<W>(
        &self,
        tiff: &mut TiffWriter<W>,
        root: &mut DirectoryWriter,
        exif_ifd: &mut DirectoryWriter,
        width: u32,
        height: u32,
    ) -> Result<()>
    where
        W: Write + Seek,
    {
        // Version stamps first: `write_exif_tags` never sets them, and readers
        // treat an EXIF IFD without ExifVersion as suspect.
        exif_ifd.add_tag_undefined(ExifTag::ExifVersion, b"0230".to_vec());
        exif_ifd.add_tag_undefined(ExifTag::FlashpixVersion, b"0100".to_vec());
        // Y, Cb, Cr, - : what a 3-channel JPEG carries. Harmless in TIFF/PNG and
        // expected by enough readers to be worth the four bytes.
        exif_ifd.add_tag_undefined(ExifTag::ComponentsConfiguration, vec![1, 2, 3, 0]);

        // Rawler's own transfer: capture settings and lens into the EXIF IFD,
        // date/artist/copyright into the root IFD, and the entire GPS IFD written
        // out and pointed at from the root. Reused rather than reimplemented
        // because the GPS block alone is thirty-odd tags of ref/value pairs.
        self.raw
            .write_exif_tags(tiff, root, exif_ifd)
            .context("failed to transfer source EXIF tags")?;

        // --- Overrides. Order matters: these run *after* the transfer above. ---

        // The orientation trap. `write_exif_tags` has just copied the source's
        // Orientation into the root IFD; the pixels have already been rotated, so
        // that value is now a lie. Overwrite it unconditionally.
        root.add_tag(ExifTag::Orientation, [ORIENTATION_NORMAL]);
        // Same reasoning for the colour space: this program decided it, the source
        // did not.
        exif_ifd.add_tag(ExifTag::ColorSpace, [COLOR_SPACE_SRGB]);

        // Dimensions of what was actually written, which for a rotated frame are
        // not the source's.
        exif_ifd.add_tag(ExifTag::ExifImageWidth, width);
        exif_ifd.add_tag(ExifTag::ExifImageHeight, height);

        if let Some(make) = self.make_string() {
            root.add_tag(TiffCommonTag::Make, make);
        }
        if let Some(model) = self.model_string() {
            root.add_tag(TiffCommonTag::Model, model);
        }
        root.add_tag(TiffCommonTag::Software, SOFTWARE);

        root.add_tag(TiffCommonTag::XResolution, Rational::new(NOMINAL_DPI, 1));
        root.add_tag(TiffCommonTag::YResolution, Rational::new(NOMINAL_DPI, 1));
        root.add_tag(TiffCommonTag::ResolutionUnit, [RESOLUTION_UNIT_INCH]);

        // Root `DateTime` (0x0132) is what several libraries sort on, and a RAW
        // often has no ModifyDate. Fall back to the capture time so the sort is
        // still chronological rather than by whenever the batch happened to run.
        if !root.contains(ExifTag::ModifyDate)
            && let Some(captured) = self.raw.exif.date_time_original.clone()
        {
            root.add_tag(ExifTag::ModifyDate, captured);
        }

        Ok(())
    }

    /// The EXIF block as a standalone TIFF structure.
    ///
    /// This is exactly what goes into a JPEG `APP1` segment after the six-byte
    /// `Exif\0\0` header, and what goes into a PNG `eXIf` chunk verbatim. All
    /// offsets inside are relative to the start of the returned buffer, which is
    /// what both containers require.
    pub fn exif_payload(&self, width: u32, height: u32) -> Result<Vec<u8>> {
        let mut buffer: Vec<u8> = Vec::new();
        {
            // `Cursor<&mut Vec<u8>>` rather than an owned Vec because
            // `TiffWriter::build` consumes the writer and does not give it back.
            let mut tiff = TiffWriter::new(Cursor::new(&mut buffer))
                .context("failed to start the EXIF TIFF structure")?;
            let mut root = DirectoryWriter::new();
            let mut exif_ifd = DirectoryWriter::new();

            self.fill(&mut tiff, &mut root, &mut exif_ifd, width, height)?;

            // Sub-IFDs must be written before the parent can point at them.
            let exif_offset = exif_ifd
                .build(&mut tiff)
                .context("failed to write the EXIF IFD")?;
            root.add_tag(TiffCommonTag::ExifIFDPointer, exif_offset);
            tiff.build(root).context("failed to write the EXIF IFD0")?;
        }
        Ok(buffer)
    }

    /// The EXIF block for a JPEG `APP1` segment, or `None` when it does not fit.
    ///
    /// A payload over 64 KB cannot be expressed as a single `APP1` segment, and
    /// splitting EXIF across segments is not a thing readers support. Nothing
    /// this module writes comes close (a GPS-tagged frame lands around 700
    /// bytes), so the cap is a guard rather than a real case; dropping the block
    /// is still better than writing a JPEG no decoder will open.
    fn jpeg_exif_payload(&self, width: u32, height: u32) -> Result<Option<Vec<u8>>> {
        let payload = self.exif_payload(width, height)?;
        Ok((payload.len() <= MAX_JPEG_EXIF_PAYLOAD).then_some(payload))
    }
}

/// Read Rawler's `RawMetadata` for `path`.
fn read_raw_metadata(path: &Path) -> Option<RawMetadata> {
    let source = RawSource::new(path).ok()?;
    let decoder = loader().get_decoder(&source).ok()?;
    decoder
        .raw_metadata(&source, &RawDecodeParams::default())
        .ok()
}

/// Set the EXIF and ICC on an `image` encoder that supports them.
///
/// JPEG and PNG both implement `set_icc_profile` and `set_exif_metadata`, so the
/// two formats need no bespoke container surgery here: `image` writes the `APP1`
/// / `APP2` segments and the `eXIf` / `iCCP` chunks itself.
fn attach<E: image::ImageEncoder>(encoder: &mut E, exif: Option<Vec<u8>>, icc: bool) -> Result<()> {
    if let Some(exif) = exif {
        encoder
            .set_exif_metadata(exif)
            .map_err(|error| anyhow::anyhow!("encoder rejected EXIF metadata: {error}"))?;
    }
    if icc {
        encoder
            .set_icc_profile(srgb_icc_profile().to_vec())
            .map_err(|error| anyhow::anyhow!("encoder rejected the ICC profile: {error}"))?;
    }
    Ok(())
}

/// Write a JPEG with EXIF (`APP1`) and the sRGB ICC profile (`APP2`).
///
/// The encoder is the `jpeg-encoder` crate rather than `image`'s, so the caller
/// can reach its progressive, optimized-Huffman and chroma-subsampling knobs
/// through [`JpegSettings`]. EXIF and ICC are attached with the crate's own
/// `add_exif_metadata`/`add_icc_profile`, which prepend the `Exif\0\0` header
/// and chunk the `ICC_PROFILE` marker exactly as the JFIF spec requires.
///
/// `rgb8` must already be 8-bit interleaved RGB; the caller owns the 16-to-8
/// reduction so that the byte-for-byte result of the no-metadata path is
/// unchanged whether or not metadata is attached.
pub fn write_jpeg(
    path: &Path,
    rgb8: &[u8],
    width: u32,
    height: u32,
    settings: &JpegSettings,
    metadata: Option<&SourceMetadata>,
) -> Result<()> {
    // `jpeg-encoder` addresses pixels with 16-bit dimensions. Every RAW this
    // program targets is far inside that (the largest, an 8160x6120 Expert RAW,
    // is well under 65535), so this is a guard rather than a real case.
    let width_u16 = u16::try_from(width)
        .with_context(|| format!("JPEG width {width} exceeds the format's 65535 limit"))?;
    let height_u16 = u16::try_from(height)
        .with_context(|| format!("JPEG height {height} exceeds the format's 65535 limit"))?;

    let file =
        File::create(path).with_context(|| format!("failed to create JPEG {}", path.display()))?;
    let mut encoder = JpegEncoder::new(BufWriter::new(file), settings.quality);

    encoder.set_progressive(settings.progressive);
    encoder.set_optimized_huffman_tables(settings.optimized_huffman);
    encoder.set_sampling_factor(sampling_factor(settings.subsampling, settings.quality));

    if let Some(metadata) = metadata {
        // The same payload the PNG/TIFF paths use; `add_exif_metadata` adds the
        // six-byte `Exif\0\0` header itself, so the raw TIFF structure goes in.
        if let Some(exif) = metadata.jpeg_exif_payload(width, height)? {
            encoder
                .add_exif_metadata(&exif)
                .map_err(|error| anyhow::anyhow!("encoder rejected EXIF metadata: {error}"))?;
        }
        encoder
            .add_icc_profile(srgb_icc_profile())
            .map_err(|error| anyhow::anyhow!("encoder rejected the ICC profile: {error}"))?;
    }

    encoder
        .encode(rgb8, width_u16, height_u16, JpegColorType::Rgb)
        .with_context(|| format!("failed to encode JPEG {}", path.display()))
}

/// Map a [`JpegSubsampling`] onto the encoder's sampling factor.
///
/// `Auto` follows the common convention: full chroma resolution at quality 90
/// and above, where subsampling artefacts start to show against the smaller
/// gain, and 4:2:0 below it.
fn sampling_factor(subsampling: JpegSubsampling, quality: u8) -> SamplingFactor {
    match subsampling {
        JpegSubsampling::S444 => SamplingFactor::F_1_1,
        JpegSubsampling::S422 => SamplingFactor::F_2_1,
        JpegSubsampling::S420 => SamplingFactor::F_2_2,
        JpegSubsampling::Auto => {
            if quality >= 90 {
                SamplingFactor::F_1_1
            } else {
                SamplingFactor::F_2_2
            }
        }
    }
}

/// Write a 16-bit PNG with EXIF (`eXIf`) and the sRGB ICC profile (`iCCP`).
pub fn write_png(path: &Path, image: &Rgb16Image, metadata: Option<&SourceMetadata>) -> Result<()> {
    let file =
        File::create(path).with_context(|| format!("failed to create PNG {}", path.display()))?;
    let mut encoder = image::codecs::png::PngEncoder::new(BufWriter::new(file));

    if let Some(metadata) = metadata {
        let exif = metadata.exif_payload(image.width(), image.height())?;
        attach(&mut encoder, Some(exif), true)?;
    }

    image
        .write_with_encoder(encoder)
        .with_context(|| format!("failed to encode PNG {}", path.display()))
}

/// Write a 16-bit uncompressed RGB TIFF with EXIF, GPS and the sRGB ICC profile.
///
/// `image`'s TIFF encoder can embed an ICC profile but has no way to write an
/// EXIF IFD, so the container is built directly with Rawler's TIFF writer. The
/// pixel layout matches what `image` produced before: uncompressed, chunky,
/// 16 bits per sample, native byte order (the TIFF header rawler writes is
/// native-endian, so the samples must be too).
pub fn write_tiff(path: &Path, image: &Rgb16Image, metadata: &SourceMetadata) -> Result<()> {
    let width = image.width();
    let height = image.height();

    let file =
        File::create(path).with_context(|| format!("failed to create TIFF {}", path.display()))?;
    let mut writer = BufWriter::new(file);

    {
        let mut tiff = TiffWriter::new(&mut writer)
            .with_context(|| format!("failed to start TIFF {}", path.display()))?;
        let mut root = DirectoryWriter::new();
        let mut exif_ifd = DirectoryWriter::new();

        // Sample data first, because the strip offsets have to be known before
        // the IFD that lists them can be built.
        let samples = image.as_raw();
        let strip_samples = width as usize * 3 * TIFF_STRIP_ROWS;
        let mut offsets: Vec<u32> = Vec::new();
        let mut counts: Vec<u32> = Vec::new();
        let mut bytes: Vec<u8> = Vec::with_capacity(strip_samples * 2);
        for strip in samples.chunks(strip_samples.max(1)) {
            bytes.clear();
            for sample in strip {
                bytes.extend_from_slice(&sample.to_ne_bytes());
            }
            let offset = tiff
                .write_data(&bytes)
                .with_context(|| format!("failed to write TIFF strip in {}", path.display()))?;
            offsets.push(offset);
            counts.push(bytes.len() as u32);
        }

        metadata.fill(&mut tiff, &mut root, &mut exif_ifd, width, height)?;

        let exif_offset = exif_ifd
            .build(&mut tiff)
            .context("failed to write the TIFF EXIF IFD")?;
        root.add_tag(TiffCommonTag::ExifIFDPointer, exif_offset);
        root.add_tag_undefined(ExifTag::IccProfile, srgb_icc_profile().to_vec());

        // Baseline TIFF layout. LONG rather than SHORT for the dimensions: a
        // 50-megapixel phone frame is wider than 65535 pixels in no orientation
        // yet, but the SHORT that Rawler's own example uses is a trap waiting for
        // the first camera that is.
        root.add_tag(TiffCommonTag::ImageWidth, width);
        root.add_tag(TiffCommonTag::ImageLength, height);
        root.add_tag(TiffCommonTag::BitsPerSample, [16_u16, 16, 16]);
        root.add_tag(TiffCommonTag::Compression, [COMPRESSION_NONE]);
        root.add_tag(TiffCommonTag::PhotometricInt, [PHOTOMETRIC_RGB]);
        root.add_tag(TiffCommonTag::StripOffsets, &offsets);
        root.add_tag(TiffCommonTag::SamplesPerPixel, [3_u16]);
        root.add_tag(TiffCommonTag::RowsPerStrip, TIFF_STRIP_ROWS as u32);
        root.add_tag(TiffCommonTag::StripByteCounts, &counts);
        root.add_tag(ExifTag::PlanarConfiguration, [PLANAR_CHUNKY]);
        root.add_tag(
            TiffCommonTag::SampleFormat,
            [SAMPLE_FORMAT_UINT, SAMPLE_FORMAT_UINT, SAMPLE_FORMAT_UINT],
        );

        tiff.build(root)
            .with_context(|| format!("failed to write TIFF directory in {}", path.display()))?;
    }

    writer
        .flush()
        .with_context(|| format!("failed to flush TIFF {}", path.display()))
}

// ---------------------------------------------------------------------------
// sRGB ICC profile
// ---------------------------------------------------------------------------

/// D50, the ICC profile connection space illuminant, in s15Fixed16.
///
/// These are the exact integers the ICC specification prints for the PCS
/// illuminant field (0x0000F6D6, 0x00010000, 0x0000D32D), used verbatim so the
/// header matches every other profile bit for bit.
const ICC_D50: [i32; 3] = [0x0000_F6D6, 0x0001_0000, 0x0000_D32D];

/// D50 as a floating-point XYZ triple, decoded from [`ICC_D50`].
///
/// Deliberately derived from the s15Fixed16 constants rather than written out as
/// 0.9642/1.0/0.8249, so the colorant columns sum to *exactly* the value the
/// header declares. There are two D50s in circulation — the ICC's legacy
/// (0.964203, 1, 0.824905) and ASTM E308's (0.964212, 1, 0.825188) — and mixing
/// them leaves a profile whose white point and colorants disagree by 0.04% in Z.
/// Small, but there is no reason to ship an inconsistency.
const ICC_D50_XYZ: [f64; 3] = [
    ICC_D50[0] as f64 / 65536.0,
    ICC_D50[1] as f64 / 65536.0,
    ICC_D50[2] as f64 / 65536.0,
];

/// sRGB primaries as CIE xy chromaticities (IEC 61966-2-1), R, G, B.
const SRGB_PRIMARIES: [[f64; 2]; 3] = [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]];
/// sRGB's white point, D65, as CIE xy.
const SRGB_WHITE: [f64; 2] = [0.3127, 0.3290];

/// The Bradford cone response matrix.
///
/// The chromatic adaptation transform every ICC implementation uses in practice;
/// the ICC specification names it for the `chad` tag.
const BRADFORD: Matrix3 = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// A 3x3 matrix, row-major.
type Matrix3 = [[f64; 3]; 3];

/// A CIE xy chromaticity as XYZ at unit luminance.
fn xy_to_xyz(xy: [f64; 2]) -> [f64; 3] {
    let [x, y] = xy;
    [x / y, 1.0, (1.0 - x - y) / y]
}

fn apply(matrix: &Matrix3, vector: [f64; 3]) -> [f64; 3] {
    let mut out = [0.0; 3];
    for (row, coefficients) in matrix.iter().enumerate() {
        out[row] = coefficients
            .iter()
            .zip(vector)
            .map(|(coefficient, value)| coefficient * value)
            .sum();
    }
    out
}

fn multiply(left: &Matrix3, right: &Matrix3) -> Matrix3 {
    let mut out = [[0.0; 3]; 3];
    for row in 0..3 {
        for column in 0..3 {
            out[row][column] = (0..3).map(|k| left[row][k] * right[k][column]).sum();
        }
    }
    out
}

/// Invert a 3x3 matrix by its adjugate. Only ever called on the two
/// well-conditioned constant matrices above.
fn invert(matrix: &Matrix3) -> Matrix3 {
    let m = matrix;
    let cofactor = |row: usize, column: usize| -> f64 {
        let rows: Vec<usize> = (0..3).filter(|value| *value != row).collect();
        let columns: Vec<usize> = (0..3).filter(|value| *value != column).collect();
        let minor = m[rows[0]][columns[0]] * m[rows[1]][columns[1]]
            - m[rows[0]][columns[1]] * m[rows[1]][columns[0]];
        if (row + column) % 2 == 0 {
            minor
        } else {
            -minor
        }
    };
    let determinant: f64 = (0..3)
        .map(|column| m[0][column] * cofactor(0, column))
        .sum();
    let mut out = [[0.0; 3]; 3];
    for (row, coefficients) in out.iter_mut().enumerate() {
        for (column, coefficient) in coefficients.iter_mut().enumerate() {
            // Indices swapped: the adjugate is the cofactor matrix transposed.
            *coefficient = cofactor(column, row) / determinant;
        }
    }
    out
}

/// The RGB-to-XYZ matrix for a set of primaries and a white point.
///
/// Standard construction: scale each primary's unit-luminance XYZ so that
/// R = G = B = 1 lands exactly on the white point.
fn rgb_to_xyz(primaries: [[f64; 2]; 3], white: [f64; 2]) -> Matrix3 {
    let columns = primaries.map(xy_to_xyz);
    let mut matrix = [[0.0; 3]; 3];
    for (row, coefficients) in matrix.iter_mut().enumerate() {
        for (column, coefficient) in coefficients.iter_mut().enumerate() {
            *coefficient = columns[column][row];
        }
    }
    let scale = apply(&invert(&matrix), xy_to_xyz(white));
    for coefficients in &mut matrix {
        for (column, coefficient) in coefficients.iter_mut().enumerate() {
            *coefficient *= scale[column];
        }
    }
    matrix
}

/// The Bradford adaptation from one white point to another: the `chad` tag.
///
/// Required alongside a D50 media white point, because without it a CMM has no
/// way to undo the adaptation baked into the colorant tags.
fn bradford_adaptation(source: [f64; 3], destination: [f64; 3]) -> Matrix3 {
    let source = apply(&BRADFORD, source);
    let destination = apply(&BRADFORD, destination);
    let mut diagonal = [[0.0; 3]; 3];
    for index in 0..3 {
        diagonal[index][index] = destination[index] / source[index];
    }
    multiply(&invert(&BRADFORD), &multiply(&diagonal, &BRADFORD))
}

/// Entries in each tone reproduction curve.
///
/// 1024 is what lcms and Skia sample sRGB at; the residual against the analytic
/// curve is below one 16-bit code, which
/// [`icc_curve_matches_render_transfer_function`] asserts.
const ICC_TRC_ENTRIES: usize = 1024;

/// Profile creation date written into the header: 2000-01-01T00:00:00Z.
///
/// A fixed constant, not the current time. Reading the clock here would make the
/// output non-reproducible, which the crate treats as a defect rather than a
/// detail.
const ICC_CREATED: [u16; 6] = [2000, 1, 1, 0, 0, 0];

const ICC_DESCRIPTION: &str = "sRGB IEC61966-2.1";
const ICC_COPYRIGHT: &str = "Generated by raw-autotune. No rights reserved.";

/// A minimal, spec-valid sRGB v2 ICC profile.
///
/// Generated rather than bundled: a matrix-shaper display profile is a couple of
/// hundred lines of structure, and generating it keeps the crate self-contained
/// with no profile of uncertain provenance or licence checked into the tree. It
/// is a v2.1 `mntr` / `RGB ` / `XYZ ` profile carrying `desc`, `cprt`, `wtpt`,
/// `chad`, `rXYZ`/`gXYZ`/`bXYZ` and a shared 1024-entry `rTRC`/`gTRC`/`bTRC`,
/// which is the full required set for that class.
///
/// Built once and cached; the bytes are identical on every call.
pub fn srgb_icc_profile() -> &'static [u8] {
    static PROFILE: OnceLock<Vec<u8>> = OnceLock::new();
    PROFILE.get_or_init(build_srgb_icc_profile)
}

/// s15Fixed16Number: a signed 16.16 fixed-point value.
fn s15_fixed16(value: f64) -> i32 {
    (value * 65536.0).round() as i32
}

/// The sRGB transfer function in the direction an ICC `curveType` needs:
/// encoded device value in, linear light out.
///
/// **This is the decoding curve, the inverse of `tone::srgb_encode`.** An ICC
/// matrix-shaper profile describes how to get *from* the device *to* the profile
/// connection space, so its `rTRC`/`gTRC`/`bTRC` linearize — roughly gamma 2.2,
/// convex — and the matrix that follows expects linear light. Sampling
/// `srgb_encode` here instead produces a profile that is structurally valid and
/// silently wrong: littleCMS renders a mid-tone of 34/255 as 170/255, because the
/// encoding curve gets applied twice. Verified by transforming through this
/// profile into a reference sRGB profile and requiring the identity.
///
/// Written in f64 rather than reusing `tone::srgb_decode` (f32, tuned for
/// millions of pixels) so the table is sampled at full precision; the two are
/// checked against each other in
/// [`icc_curve_matches_render_transfer_function`].
fn srgb_to_linear(encoded: f64) -> f64 {
    let encoded = encoded.clamp(0.0, 1.0);
    if encoded <= 0.040_449_936 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// `XYZType`: 'XYZ ', four reserved bytes, three s15Fixed16 values.
fn icc_xyz_tag(xyz: [i32; 3]) -> Vec<u8> {
    let mut data = Vec::with_capacity(20);
    data.extend_from_slice(b"XYZ ");
    data.extend_from_slice(&[0; 4]);
    for value in xyz {
        data.extend_from_slice(&value.to_be_bytes());
    }
    data
}

/// `s15Fixed16ArrayType`: 'sf32', four reserved bytes, then the values.
fn icc_sf32_tag(values: &[i32]) -> Vec<u8> {
    let mut data = Vec::with_capacity(8 + values.len() * 4);
    data.extend_from_slice(b"sf32");
    data.extend_from_slice(&[0; 4]);
    for value in values {
        data.extend_from_slice(&value.to_be_bytes());
    }
    data
}

/// `curveType`: 'curv', four reserved bytes, a count, then that many uInt16.
fn icc_curve_tag(entries: &[u16]) -> Vec<u8> {
    let mut data = Vec::with_capacity(12 + entries.len() * 2);
    data.extend_from_slice(b"curv");
    data.extend_from_slice(&[0; 4]);
    data.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for entry in entries {
        data.extend_from_slice(&entry.to_be_bytes());
    }
    data
}

/// `textType`: 'text', four reserved bytes, a NUL-terminated ASCII string.
fn icc_text_tag(text: &str) -> Vec<u8> {
    let mut data = Vec::with_capacity(9 + text.len());
    data.extend_from_slice(b"text");
    data.extend_from_slice(&[0; 4]);
    data.extend_from_slice(text.as_bytes());
    data.push(0);
    data
}

/// `textDescriptionType`, the v2-only type the `desc` tag must use.
///
/// The Unicode and ScriptCode sections are present but empty, and the 67-byte
/// Macintosh ScriptCode buffer is written even though its count is zero — the
/// type has a fixed size and readers that skip by it break otherwise.
fn icc_desc_tag(text: &str) -> Vec<u8> {
    let ascii = text.as_bytes();
    let count = ascii.len() as u32 + 1;
    let mut data = Vec::with_capacity(90 + ascii.len());
    data.extend_from_slice(b"desc");
    data.extend_from_slice(&[0; 4]);
    data.extend_from_slice(&count.to_be_bytes());
    data.extend_from_slice(ascii);
    data.push(0);
    data.extend_from_slice(&[0; 4]); // Unicode language code
    data.extend_from_slice(&[0; 4]); // Unicode count
    data.extend_from_slice(&[0; 2]); // ScriptCode code
    data.push(0); // ScriptCode count
    data.extend_from_slice(&[0; 67]); // ScriptCode description
    data
}

fn build_srgb_icc_profile() -> Vec<u8> {
    let curve: Vec<u16> = (0..ICC_TRC_ENTRIES)
        .map(|index| {
            let encoded = index as f64 / (ICC_TRC_ENTRIES - 1) as f64;
            (srgb_to_linear(encoded) * 65535.0).round() as u16
        })
        .collect();
    let curve = icc_curve_tag(&curve);

    // The colorants are the sRGB primaries taken to XYZ under their own D65
    // white, then Bradford-adapted to the profile connection space's D50. Derived
    // here rather than pasted in as a table so the chain from "sRGB primaries" to
    // "profile bytes" is visible; `icc_colorants_are_the_derived_srgb_matrix`
    // pins the resulting integers.
    let adaptation = bradford_adaptation(xy_to_xyz(SRGB_WHITE), ICC_D50_XYZ);
    let colorants = multiply(&adaptation, &rgb_to_xyz(SRGB_PRIMARIES, SRGB_WHITE));

    let colorant = |column: usize| {
        icc_xyz_tag([
            s15_fixed16(colorants[0][column]),
            s15_fixed16(colorants[1][column]),
            s15_fixed16(colorants[2][column]),
        ])
    };

    let chad: Vec<i32> = adaptation
        .iter()
        .flatten()
        .map(|value| s15_fixed16(*value))
        .collect();

    // Distinct tag payloads, in the order they will be laid out. The three TRC
    // tags share one payload, which the ICC specification explicitly permits and
    // which saves 4 KB in every output file.
    let payloads: [Vec<u8>; 7] = [
        icc_desc_tag(ICC_DESCRIPTION),
        icc_xyz_tag(ICC_D50),
        icc_sf32_tag(&chad),
        colorant(0),
        colorant(1),
        colorant(2),
        curve,
    ];
    const DESC: usize = 0;
    const WTPT: usize = 1;
    const CHAD: usize = 2;
    const RED: usize = 3;
    const GREEN: usize = 4;
    const BLUE: usize = 5;
    const TRC: usize = 6;

    let copyright = icc_text_tag(ICC_COPYRIGHT);

    // Fixed order, ascending by signature, so the tag table is byte-identical on
    // every build. `cprt` is its own payload; everything else indexes `payloads`.
    let table: [(&[u8; 4], Option<usize>); 10] = [
        (b"bTRC", Some(TRC)),
        (b"bXYZ", Some(BLUE)),
        (b"chad", Some(CHAD)),
        (b"cprt", None),
        (b"desc", Some(DESC)),
        (b"gTRC", Some(TRC)),
        (b"gXYZ", Some(GREEN)),
        (b"rTRC", Some(TRC)),
        (b"rXYZ", Some(RED)),
        (b"wtpt", Some(WTPT)),
    ];

    // Lay the payloads out after the header and tag table, each 4-byte aligned.
    let header_size = 128 + 4 + table.len() * 12;
    let mut offsets = [0_u32; 7];
    let mut body = Vec::new();
    let push = |body: &mut Vec<u8>, data: &[u8]| -> u32 {
        while body.len() % 4 != 0 {
            body.push(0);
        }
        let offset = (header_size + body.len()) as u32;
        body.extend_from_slice(data);
        offset
    };
    for (index, payload) in payloads.iter().enumerate() {
        offsets[index] = push(&mut body, payload);
    }
    let copyright_offset = push(&mut body, &copyright);
    while body.len() % 4 != 0 {
        body.push(0);
    }

    let total = header_size + body.len();
    let mut profile = Vec::with_capacity(total);

    // --- 128-byte header, all big-endian. ---
    profile.extend_from_slice(&(total as u32).to_be_bytes()); // profile size
    profile.extend_from_slice(&[0; 4]); // preferred CMM: none
    profile.extend_from_slice(&0x0210_0000_u32.to_be_bytes()); // version 2.1.0
    profile.extend_from_slice(b"mntr"); // device class: display
    profile.extend_from_slice(b"RGB "); // data colour space
    profile.extend_from_slice(b"XYZ "); // profile connection space
    for field in ICC_CREATED {
        profile.extend_from_slice(&field.to_be_bytes());
    }
    profile.extend_from_slice(b"acsp"); // file signature
    profile.extend_from_slice(&[0; 4]); // primary platform: none
    profile.extend_from_slice(&[0; 4]); // profile flags
    profile.extend_from_slice(&[0; 4]); // device manufacturer
    profile.extend_from_slice(&[0; 4]); // device model
    profile.extend_from_slice(&[0; 8]); // device attributes
    profile.extend_from_slice(&[0; 4]); // rendering intent: perceptual
    for value in ICC_D50 {
        profile.extend_from_slice(&value.to_be_bytes());
    }
    profile.extend_from_slice(&[0; 4]); // profile creator
    profile.extend_from_slice(&[0; 16]); // profile ID: v4 only
    profile.extend_from_slice(&[0; 28]); // reserved
    debug_assert_eq!(profile.len(), 128);

    // --- Tag table. ---
    profile.extend_from_slice(&(table.len() as u32).to_be_bytes());
    for (signature, payload) in table {
        let (offset, size) = match payload {
            Some(index) => (offsets[index], payloads[index].len() as u32),
            None => (copyright_offset, copyright.len() as u32),
        };
        profile.extend_from_slice(signature);
        profile.extend_from_slice(&offset.to_be_bytes());
        profile.extend_from_slice(&size.to_be_bytes());
    }
    debug_assert_eq!(profile.len(), header_size);

    profile.extend_from_slice(&body);
    debug_assert_eq!(profile.len(), total);
    profile
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgb};
    use rawler::formats::tiff::Value;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("raw-autotune-metadata-tests");
        std::fs::create_dir_all(&dir).expect("scratch directory");
        dir.join(name)
    }

    /// A source with every tag this module claims to copy, plus a rotated
    /// orientation so the override can be observed.
    fn sample_exif() -> Exif {
        let mut exif = Exif {
            // Rotate 90 CW. If this ever reaches an output file, the archive is
            // sideways.
            orientation: Some(6),
            date_time_original: Some("2026:07:30 14:03:07".to_string()),
            create_date: Some("2026:07:30 14:03:07".to_string()),
            exposure_time: Some(Rational::new(1, 250)),
            fnumber: Some(Rational::new(28, 10)),
            iso_speed_ratings: Some(400),
            focal_length: Some(Rational::new(35, 1)),
            lens_model: Some("FE 35mm F1.8".to_string()),
            lens_make: Some("Sony".to_string()),
            artist: Some("A Photographer".to_string()),
            copyright: Some("(c) 2026".to_string()),
            // The renderer never produces Adobe RGB, so this must be overridden.
            color_space: Some(2),
            ..Default::default()
        };
        exif.gps = Some(rawler::exif::ExifGPS {
            gps_version_id: Some([2, 3, 0, 0]),
            gps_latitude_ref: Some("N".to_string()),
            gps_latitude: Some([
                Rational::new(52, 1),
                Rational::new(31, 1),
                Rational::new(1234, 100),
            ]),
            gps_longitude_ref: Some("E".to_string()),
            gps_longitude: Some([
                Rational::new(13, 1),
                Rational::new(24, 1),
                Rational::new(5678, 100),
            ]),
            gps_altitude_ref: Some(0),
            gps_altitude: Some(Rational::new(34, 1)),
            ..Default::default()
        });
        exif
    }

    fn sample_metadata() -> SourceMetadata {
        SourceMetadata::from_exif(sample_exif()).with_camera("SONY", "ILCE-7C")
    }

    fn gradient(width: u32, height: u32) -> Rgb16Image {
        Rgb16Image::from_fn(width, height, |x, y| {
            let value = ((x * 257 + y * 131) % 65536) as u16;
            Rgb([value, value.wrapping_add(4096), value / 2])
        })
    }

    /// Parse a written file's EXIF back with Rawler's reader.
    fn read_back_exif(bytes: &[u8]) -> (IFD, Exif) {
        let mut cursor = Cursor::new(bytes);
        let tiff = GenericTiffReader::new(&mut cursor, 0, 0, None, &[])
            .expect("written EXIF must parse as TIFF");
        let root = tiff.root_ifd().clone();
        let exif = Exif::new(&root).expect("EXIF must parse");
        (root, exif)
    }

    #[test]
    fn exif_payload_round_trips_every_copied_field() {
        let payload = sample_metadata()
            .exif_payload(6000, 4000)
            .expect("payload builds");
        let (root, exif) = read_back_exif(&payload);

        assert_eq!(
            root.get_entry(TiffCommonTag::Make)
                .and_then(|entry| entry.value.as_string())
                .map(String::as_str),
            Some("SONY")
        );
        assert_eq!(
            root.get_entry(TiffCommonTag::Model)
                .and_then(|entry| entry.value.as_string())
                .map(String::as_str),
            Some("ILCE-7C")
        );
        assert_eq!(
            root.get_entry(TiffCommonTag::Software)
                .and_then(|entry| entry.value.as_string())
                .map(String::as_str),
            Some(SOFTWARE)
        );

        assert_eq!(
            exif.date_time_original.as_deref(),
            Some("2026:07:30 14:03:07")
        );
        assert_eq!(exif.exposure_time, Some(Rational::new(1, 250)));
        assert_eq!(exif.fnumber, Some(Rational::new(28, 10)));
        assert_eq!(exif.iso_speed_ratings, Some(400));
        assert_eq!(exif.focal_length, Some(Rational::new(35, 1)));
        assert_eq!(exif.artist.as_deref(), Some("A Photographer"));
        assert_eq!(exif.copyright.as_deref(), Some("(c) 2026"));

        // `Exif::extend_from_ifd` has no parse arm for LensModel, so read the tag
        // straight out of the EXIF IFD.
        let exif_ifd = root
            .get_sub_ifd(ExifTag::ExifOffset)
            .expect("EXIF IFD must be reachable from IFD0");
        assert_eq!(
            exif_ifd
                .get_entry(ExifTag::LensModel)
                .and_then(|entry| entry.value.as_string())
                .map(String::as_str),
            Some("FE 35mm F1.8")
        );

        // Root DateTime falls back to the capture time so a library that sorts on
        // 0x0132 still sorts chronologically.
        assert_eq!(exif.modify_date.as_deref(), Some("2026:07:30 14:03:07"));
    }

    #[test]
    fn exif_payload_carries_the_gps_block() {
        let payload = sample_metadata()
            .exif_payload(6000, 4000)
            .expect("payload builds");
        let (_, exif) = read_back_exif(&payload);
        let gps = exif.gps.expect("GPS block must survive the copy");
        assert_eq!(gps.gps_latitude_ref.as_deref(), Some("N"));
        assert_eq!(
            gps.gps_latitude,
            Some([
                Rational::new(52, 1),
                Rational::new(31, 1),
                Rational::new(1234, 100)
            ])
        );
        assert_eq!(gps.gps_longitude_ref.as_deref(), Some("E"));
        assert_eq!(gps.gps_altitude, Some(Rational::new(34, 1)));
    }

    /// The whole point of [`ORIENTATION_NORMAL`]: the pixels are already upright,
    /// so a rotated source must still produce orientation 1.
    #[test]
    fn rotated_source_is_written_as_upright() {
        let metadata = sample_metadata();
        assert_eq!(
            metadata.source_orientation(),
            Some(6),
            "the fixture must actually be a rotated frame, or this proves nothing"
        );

        let payload = metadata.exif_payload(4000, 6000).expect("payload builds");
        let (_, exif) = read_back_exif(&payload);
        assert_eq!(exif.orientation, Some(ORIENTATION_NORMAL));
    }

    #[test]
    fn output_declares_srgb_whatever_the_source_claimed() {
        let payload = sample_metadata()
            .exif_payload(6000, 4000)
            .expect("payload builds");
        let (_, exif) = read_back_exif(&payload);
        assert_eq!(exif.color_space, Some(COLOR_SPACE_SRGB));
    }

    #[test]
    fn exif_dimensions_are_the_written_ones() {
        // A portrait render of a landscape sensor: the EXIF must describe the
        // file, not the sensor.
        let payload = sample_metadata()
            .exif_payload(4000, 6000)
            .expect("payload builds");
        let (root, _) = read_back_exif(&payload);
        let exif_ifd = root.get_sub_ifd(ExifTag::ExifOffset).expect("EXIF IFD");
        assert_eq!(
            exif_ifd
                .get_entry(ExifTag::ExifImageWidth)
                .map(|entry| entry.force_u32(0)),
            Some(4000)
        );
        assert_eq!(
            exif_ifd
                .get_entry(ExifTag::ExifImageHeight)
                .map(|entry| entry.force_u32(0)),
            Some(6000)
        );
    }

    #[test]
    fn empty_source_still_produces_a_valid_block() {
        let payload = SourceMetadata::default()
            .exif_payload(64, 48)
            .expect("payload builds even with nothing to copy");
        let (root, exif) = read_back_exif(&payload);
        assert_eq!(exif.orientation, Some(ORIENTATION_NORMAL));
        assert_eq!(exif.color_space, Some(COLOR_SPACE_SRGB));
        assert_eq!(
            root.get_entry(TiffCommonTag::Software)
                .and_then(|entry| entry.value.as_string())
                .map(String::as_str),
            Some(SOFTWARE)
        );
        assert!(SourceMetadata::default().is_empty());
    }

    #[test]
    fn exif_payload_is_byte_identical_across_calls() {
        let metadata = sample_metadata();
        let first = metadata.exif_payload(6000, 4000).expect("payload builds");
        let second = metadata.exif_payload(6000, 4000).expect("payload builds");
        assert_eq!(first, second);
        // A separately constructed instance of the same source must agree too:
        // nothing may depend on allocation addresses or map iteration order.
        let third = sample_metadata()
            .exif_payload(6000, 4000)
            .expect("payload builds");
        assert_eq!(first, third);
    }

    #[test]
    fn jpeg_round_trips_exif_and_icc() {
        let path = scratch("metadata.jpg");
        let image = gradient(32, 24);
        let rgb8: Vec<u8> = image
            .as_raw()
            .iter()
            .map(|value| ((*value as u32 + 128) / 257) as u8)
            .collect();
        write_jpeg(
            &path,
            &rgb8,
            32,
            24,
            &JpegSettings::from_quality(90),
            Some(&sample_metadata()),
        )
        .expect("JPEG writes");

        let bytes = std::fs::read(&path).expect("JPEG readable");
        let exif = extract_jpeg_app1(&bytes).expect("APP1 Exif segment must be present");
        let (root, parsed) = read_back_exif(&exif);
        assert_eq!(parsed.orientation, Some(ORIENTATION_NORMAL));
        assert_eq!(parsed.iso_speed_ratings, Some(400));
        assert_eq!(
            root.get_entry(TiffCommonTag::Model)
                .and_then(|entry| entry.value.as_string())
                .map(String::as_str),
            Some("ILCE-7C")
        );

        let icc = extract_jpeg_app2(&bytes).expect("APP2 ICC_PROFILE segment must be present");
        assert_eq!(icc, srgb_icc_profile());

        // And the pixels still decode.
        let decoded = image::open(&path).expect("JPEG decodes");
        assert_eq!((decoded.width(), decoded.height()), (32, 24));
    }

    #[test]
    fn jpeg_without_metadata_has_no_app1_or_app2() {
        let path = scratch("metadata-none.jpg");
        let image = gradient(32, 24);
        let rgb8: Vec<u8> = image
            .as_raw()
            .iter()
            .map(|value| ((*value as u32 + 128) / 257) as u8)
            .collect();
        write_jpeg(&path, &rgb8, 32, 24, &JpegSettings::from_quality(90), None)
            .expect("JPEG writes");
        let bytes = std::fs::read(&path).expect("JPEG readable");
        assert!(extract_jpeg_app1(&bytes).is_none());
        assert!(extract_jpeg_app2(&bytes).is_none());
    }

    #[test]
    fn tiff_round_trips_exif_icc_and_pixels() {
        let path = scratch("metadata.tif");
        let image = gradient(37, 21);
        write_tiff(&path, &image, &sample_metadata()).expect("TIFF writes");

        let bytes = std::fs::read(&path).expect("TIFF readable");
        let (root, parsed) = read_back_exif(&bytes);
        assert_eq!(parsed.orientation, Some(ORIENTATION_NORMAL));
        assert_eq!(parsed.color_space, Some(COLOR_SPACE_SRGB));
        assert_eq!(parsed.exposure_time, Some(Rational::new(1, 250)));
        assert!(parsed.gps.is_some(), "GPS must reach the TIFF too");

        match root
            .get_entry(ExifTag::IccProfile)
            .map(|entry| &entry.value)
        {
            Some(Value::Undefined(profile)) => assert_eq!(profile, srgb_icc_profile()),
            other => panic!("expected an UNDEFINED ICC profile tag, got {other:?}"),
        }

        // The pixels must survive a third-party TIFF decoder unchanged, which is
        // the real check that the strip layout is right.
        let decoded = image::open(&path).expect("TIFF decodes");
        let decoded = match decoded {
            DynamicImage::ImageRgb16(decoded) => decoded,
            other => panic!("expected RGB16, got {:?}", other.color()),
        };
        assert_eq!(decoded.dimensions(), image.dimensions());
        assert_eq!(decoded.as_raw(), image.as_raw());
    }

    /// More rows than [`TIFF_STRIP_ROWS`], so `StripOffsets`/`StripByteCounts`
    /// become real arrays with a short final strip. Every earlier TIFF test fits
    /// in one strip and would not notice an off-by-one here.
    #[test]
    fn tiff_multi_strip_round_trips_pixels() {
        let path = scratch("metadata-strips.tif");
        let height = TIFF_STRIP_ROWS as u32 * 2 + 7;
        let image = gradient(23, height);
        write_tiff(&path, &image, &sample_metadata()).expect("TIFF writes");

        let bytes = std::fs::read(&path).expect("TIFF readable");
        let (root, _) = read_back_exif(&bytes);
        let offsets = root
            .get_entry(TiffCommonTag::StripOffsets)
            .expect("StripOffsets");
        assert_eq!(offsets.count(), 3, "expected three strips");
        let counts = root
            .get_entry(TiffCommonTag::StripByteCounts)
            .expect("StripByteCounts");
        assert_eq!(counts.count(), 3);
        assert_eq!(
            counts.force_u32(2),
            23 * 3 * 7 * 2,
            "the final strip must be the 7 leftover rows, not a full 64"
        );

        let decoded = image::open(&path).expect("TIFF decodes");
        let decoded = match decoded {
            DynamicImage::ImageRgb16(decoded) => decoded,
            other => panic!("expected RGB16, got {:?}", other.color()),
        };
        assert_eq!(decoded.dimensions(), (23, height));
        assert_eq!(decoded.as_raw(), image.as_raw());
    }

    #[test]
    fn tiff_is_byte_identical_across_runs() {
        let metadata = sample_metadata();
        let image = gradient(19, 33);
        let first = scratch("determinism-a.tif");
        let second = scratch("determinism-b.tif");
        write_tiff(&first, &image, &metadata).expect("TIFF writes");
        write_tiff(&second, &image, &metadata).expect("TIFF writes");
        assert_eq!(
            std::fs::read(&first).expect("readable"),
            std::fs::read(&second).expect("readable")
        );
    }

    #[test]
    fn png_round_trips_exif_and_icc() {
        let path = scratch("metadata.png");
        let image = gradient(24, 18);
        write_png(&path, &image, Some(&sample_metadata())).expect("PNG writes");
        let bytes = std::fs::read(&path).expect("PNG readable");

        let exif = extract_png_chunk(&bytes, b"eXIf").expect("eXIf chunk must be present");
        let (_, parsed) = read_back_exif(&exif);
        assert_eq!(parsed.orientation, Some(ORIENTATION_NORMAL));
        assert_eq!(parsed.iso_speed_ratings, Some(400));

        // iCCP is zlib-compressed, so only its presence and profile name are
        // checked here; the profile bytes are verified through the JPEG and TIFF
        // paths, which store it uncompressed.
        let iccp = extract_png_chunk(&bytes, b"iCCP").expect("iCCP chunk must be present");
        assert!(iccp.contains(&0), "iCCP must carry a NUL-terminated name");

        let decoded = image::open(&path).expect("PNG decodes");
        assert_eq!((decoded.width(), decoded.height()), (24, 18));
    }

    #[test]
    fn png_without_metadata_has_no_exif_or_icc() {
        let path = scratch("metadata-none.png");
        let image = gradient(24, 18);
        write_png(&path, &image, None).expect("PNG writes");
        let bytes = std::fs::read(&path).expect("PNG readable");
        assert!(extract_png_chunk(&bytes, b"eXIf").is_none());
        assert!(extract_png_chunk(&bytes, b"iCCP").is_none());
    }

    #[test]
    fn non_ascii_and_control_characters_are_stripped() {
        assert_eq!(ascii("  Sony \u{fffd}\u{0007}"), Some("Sony".to_string()));
        assert_eq!(ascii("\u{0000}"), None);
        assert_eq!(ascii("   "), None);
        assert_eq!(ascii("FE 35mm F1.8"), Some("FE 35mm F1.8".to_string()));
    }

    #[test]
    fn interior_nul_cannot_reach_the_tiff_writer() {
        let exif = Exif {
            artist: Some("bad\u{0000}name".to_string()),
            lens_model: Some("lens\u{0000}".to_string()),
            ..Default::default()
        };
        let payload = SourceMetadata::from_exif(exif)
            .exif_payload(8, 8)
            .expect("payload builds without panicking");
        let (_, parsed) = read_back_exif(&payload);
        assert_eq!(parsed.artist.as_deref(), Some("badname"));
    }

    // --- ICC profile ---

    #[test]
    fn icc_profile_header_is_self_consistent() {
        let profile = srgb_icc_profile();
        assert!(profile.len() > 128 + 4);
        let declared = u32::from_be_bytes(profile[0..4].try_into().unwrap()) as usize;
        assert_eq!(declared, profile.len(), "header size must match the bytes");
        assert_eq!(&profile[12..16], b"mntr");
        assert_eq!(&profile[16..20], b"RGB ");
        assert_eq!(&profile[20..24], b"XYZ ");
        assert_eq!(&profile[36..40], b"acsp");
        assert_eq!(
            u32::from_be_bytes(profile[8..12].try_into().unwrap()),
            0x0210_0000,
            "must declare ICC v2.1"
        );
        // PCS illuminant is D50, as the specification requires.
        for (index, expected) in ICC_D50.iter().enumerate() {
            let start = 68 + index * 4;
            let value = i32::from_be_bytes(profile[start..start + 4].try_into().unwrap());
            assert_eq!(value, *expected);
        }
    }

    #[test]
    fn icc_tag_table_is_complete_sorted_and_in_bounds() {
        let profile = srgb_icc_profile();
        let count = u32::from_be_bytes(profile[128..132].try_into().unwrap()) as usize;
        assert_eq!(count, 10);

        let mut signatures = Vec::new();
        for index in 0..count {
            let entry = 132 + index * 12;
            let signature: [u8; 4] = profile[entry..entry + 4].try_into().unwrap();
            let offset = u32::from_be_bytes(profile[entry + 4..entry + 8].try_into().unwrap());
            let size = u32::from_be_bytes(profile[entry + 8..entry + 12].try_into().unwrap());
            assert!(
                offset as usize + size as usize <= profile.len(),
                "tag {:?} runs past the end of the profile",
                std::str::from_utf8(&signature)
            );
            assert_eq!(offset % 4, 0, "tag data must be 4-byte aligned");
            signatures.push(signature);
        }

        let mut sorted = signatures.clone();
        sorted.sort_unstable();
        assert_eq!(
            signatures, sorted,
            "the tag table must be emitted in a fixed sorted order"
        );

        for required in [
            b"desc", b"cprt", b"wtpt", b"chad", b"rXYZ", b"gXYZ", b"bXYZ", b"rTRC", b"gTRC",
            b"bTRC",
        ] {
            assert!(
                signatures.contains(required),
                "a v2 matrix-shaper profile requires {:?}",
                std::str::from_utf8(required)
            );
        }
    }

    fn derived_colorants() -> Matrix3 {
        multiply(
            &bradford_adaptation(xy_to_xyz(SRGB_WHITE), ICC_D50_XYZ),
            &rgb_to_xyz(SRGB_PRIMARIES, SRGB_WHITE),
        )
    }

    /// The colorant columns must sum to the media white point, or a CMM renders
    /// neutral grey with a cast.
    ///
    /// Exact equality in s15Fixed16, not a tolerance: the derivation adapts to
    /// the same D50 the header declares, so there is nothing left to round away.
    #[test]
    fn srgb_matrix_columns_sum_to_d50() {
        let colorants = derived_colorants();
        for (row, expected) in ICC_D50.iter().enumerate() {
            let sum: f64 = colorants[row].iter().sum();
            assert_eq!(
                s15_fixed16(sum),
                *expected,
                "colorant row {row} must sum to the declared white point"
            );
        }
    }

    /// Pin the derived numbers, independently computed with NumPy from the
    /// IEC 61966-2-1 primaries and the Bradford transform. If the linear algebra
    /// above ever drifts, this catches it before a profile ships.
    #[test]
    fn icc_colorants_are_the_derived_srgb_matrix() {
        let colorants = derived_colorants();
        let encoded: Vec<Vec<i32>> = colorants
            .iter()
            .map(|row| row.iter().map(|value| s15_fixed16(*value)).collect())
            .collect();
        assert_eq!(
            encoded,
            vec![
                vec![28576, 25239, 9375],
                vec![14581, 46983, 3972],
                vec![912, 6361, 46787],
            ]
        );

        let chad: Vec<i32> = bradford_adaptation(xy_to_xyz(SRGB_WHITE), ICC_D50_XYZ)
            .iter()
            .flatten()
            .map(|value| s15_fixed16(*value))
            .collect();
        assert_eq!(
            chad,
            vec![68674, 1502, -3291, 1939, 64912, -1119, -606, 988, 49262]
        );
    }

    #[test]
    fn matrix_inversion_round_trips() {
        let matrix = rgb_to_xyz(SRGB_PRIMARIES, SRGB_WHITE);
        let identity = multiply(&matrix, &invert(&matrix));
        for (row, coefficients) in identity.iter().enumerate() {
            for (column, coefficient) in coefficients.iter().enumerate() {
                let expected = if row == column { 1.0 } else { 0.0 };
                assert!(
                    (coefficient - expected).abs() < 1e-12,
                    "inverse is wrong at {row},{column}: {coefficient}"
                );
            }
        }
        // And R=G=B=1 must land on the white point it was built from.
        let white = apply(&matrix, [1.0, 1.0, 1.0]);
        let expected = xy_to_xyz(SRGB_WHITE);
        for index in 0..3 {
            assert!((white[index] - expected[index]).abs() < 1e-12);
        }
    }

    /// The profile's curve must be the exact inverse of the transfer function the
    /// renderer applies, because that is the direction ICC defines it in. If
    /// `tone::srgb_encode` ever changes, this fails.
    #[test]
    fn icc_curve_matches_render_transfer_function() {
        let profile = srgb_icc_profile();
        let count = u32::from_be_bytes(profile[128..132].try_into().unwrap()) as usize;
        let mut trc = None;
        for index in 0..count {
            let entry = 132 + index * 12;
            if &profile[entry..entry + 4] == b"rTRC" {
                let offset =
                    u32::from_be_bytes(profile[entry + 4..entry + 8].try_into().unwrap()) as usize;
                let size =
                    u32::from_be_bytes(profile[entry + 8..entry + 12].try_into().unwrap()) as usize;
                trc = Some(&profile[offset..offset + size]);
            }
        }
        let trc = trc.expect("rTRC must exist");
        assert_eq!(&trc[0..4], b"curv");
        let entries = u32::from_be_bytes(trc[8..12].try_into().unwrap()) as usize;
        assert_eq!(entries, ICC_TRC_ENTRIES);

        let mut previous = 0_u16;
        for index in 0..entries {
            let start = 12 + index * 2;
            let value = u16::from_be_bytes(trc[start..start + 2].try_into().unwrap());
            assert!(value >= previous, "the curve must be monotonic");
            previous = value;

            // The renderer encodes; the profile decodes. Feed the curve's own
            // output back through `srgb_encode` and it must land on the input.
            let encoded = index as f32 / (entries - 1) as f32;
            let linear = value as f32 / 65535.0;
            let round_tripped = crate::tone::srgb_encode(linear);
            assert!(
                (round_tripped - encoded).abs() < 1.0 / 2048.0,
                "curve entry {index} decodes {encoded} to {linear}, which the \
                 renderer re-encodes as {round_tripped}"
            );
        }
        assert_eq!(previous, 65535, "the curve must reach full scale");

        // Sanity on the direction itself: a decoding curve is convex, so the
        // mid-point must sit well below half scale. An encoding curve would put it
        // well above, which is the bug this guards.
        let midpoint = 12 + (entries / 2) * 2;
        let middle = u16::from_be_bytes(trc[midpoint..midpoint + 2].try_into().unwrap());
        assert!(
            middle < 16384,
            "the TRC looks like an encoding curve, not the decoding curve ICC wants: \
             mid-point is {middle} of 65535"
        );
    }

    #[test]
    fn icc_profile_is_cached_and_stable() {
        let first = srgb_icc_profile();
        let second = srgb_icc_profile();
        assert_eq!(first.as_ptr(), second.as_ptr());
        assert_eq!(first, build_srgb_icc_profile().as_slice());
    }

    // --- container helpers, test-only ---

    /// Pull the EXIF payload out of a JPEG's `APP1` segment.
    fn extract_jpeg_app1(bytes: &[u8]) -> Option<Vec<u8>> {
        for (marker, payload) in jpeg_segments(bytes) {
            if marker == 0xE1 && payload.starts_with(b"Exif\0\0") {
                return Some(payload[6..].to_vec());
            }
        }
        None
    }

    /// Pull the ICC profile out of a JPEG's `APP2` segments.
    fn extract_jpeg_app2(bytes: &[u8]) -> Option<Vec<u8>> {
        let mut profile = Vec::new();
        for (marker, payload) in jpeg_segments(bytes) {
            if marker == 0xE2 && payload.starts_with(b"ICC_PROFILE\0") {
                profile.extend_from_slice(&payload[14..]);
            }
        }
        (!profile.is_empty()).then_some(profile)
    }

    fn jpeg_segments(bytes: &[u8]) -> Vec<(u8, &[u8])> {
        let mut segments = Vec::new();
        let mut index = 2; // skip SOI
        while index + 4 <= bytes.len() && bytes[index] == 0xFF {
            let marker = bytes[index + 1];
            if marker == 0xD8 || marker == 0xD9 || marker == 0xDA {
                break;
            }
            let length = u16::from_be_bytes([bytes[index + 2], bytes[index + 3]]) as usize;
            if length < 2 || index + 2 + length > bytes.len() {
                break;
            }
            segments.push((marker, &bytes[index + 4..index + 2 + length]));
            index += 2 + length;
        }
        segments
    }

    /// Pull a named chunk out of a PNG.
    fn extract_png_chunk(bytes: &[u8], name: &[u8; 4]) -> Option<Vec<u8>> {
        let mut index = 8; // skip the signature
        while index + 12 <= bytes.len() {
            let length = u32::from_be_bytes(bytes[index..index + 4].try_into().unwrap()) as usize;
            let kind = &bytes[index + 4..index + 8];
            if kind == name {
                return Some(bytes[index + 8..index + 8 + length].to_vec());
            }
            if kind == b"IEND" {
                break;
            }
            index += 12 + length;
        }
        None
    }

    /// Read metadata out of the real corpus when it is present.
    ///
    /// `raw/` is not distributable, so this skips rather than fails when the
    /// directory is absent.
    fn find_source(extension: &str) -> Option<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("raw");
        if !root.is_dir() {
            return None;
        }
        let mut found: Vec<PathBuf> = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case(extension))
                {
                    found.push(path);
                }
            }
        }
        // Sorted so the test picks the same file every run.
        found.sort();
        found.into_iter().next()
    }

    #[test]
    fn real_raw_metadata_survives_a_jpeg_round_trip() {
        let Some(source) = find_source("arw").or_else(|| find_source("dng")) else {
            eprintln!("skipping: no RAW corpus under raw/");
            return;
        };

        let metadata = SourceMetadata::read(&source);
        assert!(
            !metadata.is_empty(),
            "{} yielded no metadata at all",
            source.display()
        );
        let captured = metadata
            .date_time_original()
            .unwrap_or_default()
            .to_string();
        assert!(
            captured.len() >= 19,
            "{} has no usable DateTimeOriginal: {captured:?}",
            source.display()
        );

        let path = scratch("real-source.jpg");
        let image = gradient(64, 48);
        let rgb8: Vec<u8> = image
            .as_raw()
            .iter()
            .map(|value| ((*value as u32 + 128) / 257) as u8)
            .collect();
        write_jpeg(
            &path,
            &rgb8,
            64,
            48,
            &JpegSettings::from_quality(90),
            Some(&metadata),
        )
        .expect("JPEG writes");

        let bytes = std::fs::read(&path).expect("JPEG readable");
        let exif = extract_jpeg_app1(&bytes).expect("APP1 Exif segment");
        let (root, parsed) = read_back_exif(&exif);

        assert_eq!(
            parsed.date_time_original.as_deref(),
            Some(captured.as_str())
        );
        assert_eq!(parsed.orientation, Some(ORIENTATION_NORMAL));
        assert_eq!(parsed.color_space, Some(COLOR_SPACE_SRGB));
        assert!(
            root.get_entry(TiffCommonTag::Make).is_some(),
            "a real camera file must yield a Make"
        );
        assert!(
            root.get_entry(TiffCommonTag::Model).is_some(),
            "a real camera file must yield a Model"
        );
        assert!(
            parsed.exposure_time.is_some() || parsed.fnumber.is_some(),
            "a real camera file must yield at least one exposure setting"
        );
    }

    #[test]
    fn missing_files_do_not_panic() {
        let metadata = SourceMetadata::read(Path::new("no-such-file.ARW"));
        assert!(metadata.is_empty());
        assert!(metadata.exif_payload(8, 8).is_ok());
    }
}
