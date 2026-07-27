//! Lossless JPEG (SOF3) decoding with restart-marker support.
//!
//! Rawler 0.7.2's lossless JPEG decoder ignores the `DRI` marker and its bit
//! pump never resynchronizes on the `RSTn` markers that follow. A stream that
//! uses restart intervals therefore desynchronizes at the first marker and the
//! predictor runs away, producing a smooth diagonal ramp instead of an image.
//! Samsung's linear DNGs are written this way (restart every 16 rows), so they
//! decode to garbage rather than failing loudly.
//!
//! This module implements the lossless process of ITU-T T.81 Annex H directly,
//! including restart handling, so those files can be decoded correctly. It is
//! only used for streams Rawler would get wrong; see [`uses_restart_intervals`].
//!
//! Only non-subsampled scans (every component `H=V=1`) are supported, which is
//! what DNG's lossless JPEG profile uses.

use anyhow::{Result, bail, ensure};

const MARKER_SOI: u8 = 0xd8;
const MARKER_EOI: u8 = 0xd9;
const MARKER_SOF3: u8 = 0xc3;
const MARKER_DHT: u8 = 0xc4;
const MARKER_SOS: u8 = 0xda;
const MARKER_DRI: u8 = 0xdd;
const MARKER_DQT: u8 = 0xdb;

/// Geometry of a decoded lossless JPEG stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfo {
    /// Pixel columns per line, as recorded in SOF3.
    pub width: usize,
    /// Lines in this stream.
    pub height: usize,
    /// Components interleaved per pixel.
    pub components: usize,
    /// Sample precision in bits.
    pub precision: u32,
    /// MCUs between restart markers; 0 when the stream has no `DRI`.
    pub restart_interval: usize,
}

impl StreamInfo {
    /// Samples per decoded line (`width * components`).
    pub const fn samples_per_line(&self) -> usize {
        self.width * self.components
    }
}

/// Canonical JPEG Huffman table, decoded with the T.81 F.2.2.3 procedure.
#[derive(Debug, Clone, Default)]
struct HuffTable {
    /// `mincode[l]` / `maxcode[l]` bound the codes of length `l` (1..=16).
    mincode: [i32; 17],
    maxcode: [i32; 17],
    valptr: [usize; 17],
    values: Vec<u8>,
}

impl HuffTable {
    fn new(counts: &[u8; 16], values: Vec<u8>) -> Result<Self> {
        let mut table = Self {
            mincode: [0; 17],
            maxcode: [-1; 17],
            valptr: [0; 17],
            values,
        };

        let mut code: i32 = 0;
        let mut index: usize = 0;
        for length in 1..=16_usize {
            let count = counts[length - 1] as usize;
            if count == 0 {
                table.maxcode[length] = -1;
                code <<= 1;
                continue;
            }
            table.valptr[length] = index;
            table.mincode[length] = code;
            index += count;
            code += count as i32;
            table.maxcode[length] = code - 1;
            code <<= 1;
            ensure!(
                index <= table.values.len(),
                "Huffman table declares more codes ({index}) than values ({})",
                table.values.len()
            );
        }

        Ok(table)
    }
}

/// Entropy-coded segment reader.
///
/// Handles `0xFF 0x00` byte stuffing. On reaching any real marker it stops
/// consuming input and supplies zero bits, leaving `position` on the `0xFF` so
/// [`BitReader::restart`] can find the marker.
struct BitReader<'a> {
    data: &'a [u8],
    position: usize,
    bits: u32,
    count: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            position: 0,
            bits: 0,
            count: 0,
        }
    }

    #[inline]
    fn fill(&mut self) {
        while self.count <= 24 {
            let byte = match self.data.get(self.position) {
                Some(0xff) => match self.data.get(self.position + 1) {
                    // Stuffed 0xFF: consume both bytes, deliver one 0xFF.
                    Some(0x00) => {
                        self.position += 2;
                        0xff
                    }
                    // A real marker. Stop here and pad with zero bits.
                    _ => 0x00,
                },
                Some(byte) => {
                    self.position += 1;
                    *byte
                }
                None => 0x00,
            };
            self.bits = (self.bits << 8) | byte as u32;
            self.count += 8;
        }
    }

    #[inline]
    fn get_bit(&mut self) -> u32 {
        if self.count == 0 {
            self.fill();
        }
        self.count -= 1;
        (self.bits >> self.count) & 1
    }

    #[inline]
    fn get_bits(&mut self, length: u32) -> u32 {
        if length == 0 {
            return 0;
        }
        if self.count < length {
            self.fill();
        }
        self.count -= length;
        (self.bits >> self.count) & ((1 << length) - 1)
    }

    #[inline]
    fn decode_huffman(&mut self, table: &HuffTable) -> Result<u8> {
        let mut code = self.get_bit() as i32;
        for length in 1..=16_usize {
            if table.maxcode[length] >= 0 && code <= table.maxcode[length] {
                let offset = table.valptr[length] + (code - table.mincode[length]) as usize;
                return table.values.get(offset).copied().ok_or_else(|| {
                    anyhow::anyhow!("Huffman code resolved outside the value table")
                });
            }
            code = (code << 1) | self.get_bit() as i32;
        }
        bail!("no Huffman code matched within 16 bits; the entropy stream is desynchronized")
    }

    /// Discard buffered bits and skip past the next `RSTn` marker.
    fn restart(&mut self) -> Result<()> {
        self.bits = 0;
        self.count = 0;

        while self.position + 1 < self.data.len() {
            if self.data[self.position] == 0xff {
                let marker = self.data[self.position + 1];
                if (0xd0..=0xd7).contains(&marker) {
                    self.position += 2;
                    return Ok(());
                }
                if marker == MARKER_EOI {
                    bail!("reached EOI while looking for a restart marker");
                }
            }
            self.position += 1;
        }

        bail!("ran out of data while looking for a restart marker")
    }
}

/// Sign-extend a `length`-bit magnitude, per T.81 the `EXTEND` procedure.
#[inline]
fn extend(value: i32, length: u32) -> i32 {
    if length == 0 {
        return 0;
    }
    if value < (1 << (length - 1)) {
        value - (1 << length) + 1
    } else {
        value
    }
}

struct Header {
    info: StreamInfo,
    /// Huffman table selector per component, in scan order.
    table_for_component: Vec<usize>,
    tables: Vec<HuffTable>,
    predictor: u8,
    point_transform: u32,
    /// Offset of the entropy-coded data, just past the SOS segment.
    scan_start: usize,
}

/// Parse the marker segments up to and including SOS.
fn parse_header(stream: &[u8]) -> Result<Header> {
    ensure!(
        stream.len() > 4 && stream[0] == 0xff && stream[1] == MARKER_SOI,
        "not a JPEG stream (missing SOI)"
    );

    let mut position = 2usize;
    let mut tables: Vec<HuffTable> = vec![HuffTable::default(); 4];
    let mut width = 0usize;
    let mut height = 0usize;
    let mut components = 0usize;
    let mut precision = 0u32;
    let mut restart_interval = 0usize;
    let mut saw_sof3 = false;

    loop {
        // Markers may be preceded by fill bytes.
        while position < stream.len() && stream[position] != 0xff {
            position += 1;
        }
        while position < stream.len() && stream[position] == 0xff {
            position += 1;
        }
        ensure!(position < stream.len(), "stream ended before SOS");

        let marker = stream[position];
        position += 1;

        if marker == MARKER_EOI {
            bail!("reached EOI before SOS");
        }
        if marker == MARKER_DQT {
            bail!("found DQT: this is a lossy JPEG, not a lossless (SOF3) stream");
        }

        ensure!(position + 1 < stream.len(), "truncated marker segment");
        let length = ((stream[position] as usize) << 8) | stream[position + 1] as usize;
        ensure!(length >= 2, "marker segment declares an impossible length");
        let segment_start = position + 2;
        let segment_end = position + length;
        ensure!(
            segment_end <= stream.len(),
            "marker segment runs past the stream"
        );
        let segment = &stream[segment_start..segment_end];

        match marker {
            MARKER_SOF3 => {
                ensure!(segment.len() >= 6, "truncated SOF3 segment");
                precision = segment[0] as u32;
                height = ((segment[1] as usize) << 8) | segment[2] as usize;
                width = ((segment[3] as usize) << 8) | segment[4] as usize;
                components = segment[5] as usize;
                ensure!(
                    (2..=16).contains(&precision),
                    "unsupported sample precision {precision}"
                );
                ensure!(
                    (1..=4).contains(&components),
                    "unsupported component count {components}"
                );
                ensure!(
                    segment.len() >= 6 + components * 3,
                    "truncated SOF3 component list"
                );
                for index in 0..components {
                    let sampling = segment[6 + index * 3 + 1];
                    let (horizontal, vertical) = (sampling >> 4, sampling & 0x0f);
                    ensure!(
                        horizontal == 1 && vertical == 1,
                        "subsampled lossless JPEG is not supported (component {index} is {horizontal}x{vertical})"
                    );
                }
                saw_sof3 = true;
            }
            MARKER_DHT => {
                // A DHT segment may carry several tables back to back.
                let mut offset = 0usize;
                while offset < segment.len() {
                    ensure!(offset + 17 <= segment.len(), "truncated DHT segment");
                    let class_and_slot = segment[offset];
                    let slot = (class_and_slot & 0x0f) as usize;
                    ensure!(slot < 4, "DHT declares table slot {slot}");
                    let mut counts = [0u8; 16];
                    counts.copy_from_slice(&segment[offset + 1..offset + 17]);
                    let total: usize = counts.iter().map(|c| *c as usize).sum();
                    ensure!(
                        offset + 17 + total <= segment.len(),
                        "DHT value list runs past the segment"
                    );
                    let values = segment[offset + 17..offset + 17 + total].to_vec();
                    tables[slot] = HuffTable::new(&counts, values)?;
                    offset += 17 + total;
                }
            }
            MARKER_DRI => {
                ensure!(segment.len() >= 2, "truncated DRI segment");
                restart_interval = ((segment[0] as usize) << 8) | segment[1] as usize;
            }
            MARKER_SOS => {
                ensure!(saw_sof3, "reached SOS without a SOF3 frame header");
                ensure!(!segment.is_empty(), "truncated SOS segment");
                let scan_components = segment[0] as usize;
                ensure!(
                    scan_components == components,
                    "scan covers {scan_components} of {components} components; \
                     multi-scan lossless JPEG is not supported"
                );
                ensure!(
                    segment.len() >= 1 + scan_components * 2 + 3,
                    "truncated SOS parameter list"
                );

                let mut table_for_component = Vec::with_capacity(scan_components);
                for index in 0..scan_components {
                    let selector = segment[1 + index * 2 + 1];
                    table_for_component.push((selector >> 4) as usize);
                }

                let tail = 1 + scan_components * 2;
                let predictor = segment[tail];
                let point_transform = (segment[tail + 2] & 0x0f) as u32;
                ensure!(
                    (1..=7).contains(&predictor),
                    "unsupported lossless predictor {predictor}"
                );

                return Ok(Header {
                    info: StreamInfo {
                        width,
                        height,
                        components,
                        precision,
                        restart_interval,
                    },
                    table_for_component,
                    tables,
                    predictor,
                    point_transform,
                    scan_start: segment_end,
                });
            }
            _ => {}
        }

        position = segment_end;
    }
}

/// Read a stream's geometry without decoding its entropy data.
pub fn read_stream_info(stream: &[u8]) -> Result<StreamInfo> {
    Ok(parse_header(stream)?.info)
}

/// True when this stream uses restart intervals, i.e. when Rawler will mis-decode it.
pub fn uses_restart_intervals(stream: &[u8]) -> bool {
    read_stream_info(stream).is_ok_and(|info| info.restart_interval > 0)
}

/// Decode `stream`, writing its samples into `destination`.
///
/// `destination` is a sample buffer with `destination_stride` samples per row.
/// The stream's top-left sample lands at row `row_origin`, sample column
/// `sample_origin`. Rows and samples that fall outside `destination` are
/// dropped, which is what padded tiles at an image edge require.
pub fn decode_into(
    stream: &[u8],
    destination: &mut [u16],
    destination_stride: usize,
    row_origin: usize,
    sample_origin: usize,
) -> Result<StreamInfo> {
    let header = parse_header(stream)?;
    let info = header.info;
    let components = info.components;
    let samples_per_line = info.samples_per_line();

    ensure!(
        samples_per_line > 0 && info.height > 0,
        "lossless JPEG stream has an empty frame"
    );
    for selector in &header.table_for_component {
        ensure!(
            *selector < header.tables.len() && !header.tables[*selector].values.is_empty(),
            "scan references Huffman table {selector}, which the stream never defined"
        );
    }

    let mut reader = BitReader::new(&stream[header.scan_start..]);
    let default_prediction: i32 = 1 << (info.precision - header.point_transform - 1);
    let sample_mask: i32 = (1 << info.precision) - 1;

    // One decoded line, plus the previous line for the vertical predictors.
    let mut previous = vec![0u16; samples_per_line];
    let mut current = vec![0u16; samples_per_line];

    let mut mcus_until_restart = info.restart_interval;
    // Set at the start of the scan and after every restart: the next sample uses
    // `default_prediction`, and the rest of that line uses Ra (T.81 H.2.1).
    let mut interval_start = true;
    let mut interval_first_line = true;

    for row in 0..info.height {
        if row > 0 {
            std::mem::swap(&mut previous, &mut current);
        }

        for column in 0..info.width {
            if info.restart_interval > 0 && mcus_until_restart == 0 {
                reader.restart()?;
                mcus_until_restart = info.restart_interval;
                interval_start = true;
                interval_first_line = true;
            }

            for component in 0..components {
                let index = column * components + component;

                let prediction: i32 = if interval_start {
                    default_prediction
                } else if interval_first_line {
                    // Rest of the interval's first line always predicts from Ra.
                    current[index - components] as i32
                } else if column == 0 {
                    // Start of a later line always predicts from Rb.
                    previous[index] as i32
                } else {
                    let ra = current[index - components] as i32;
                    let rb = previous[index] as i32;
                    let rc = previous[index - components] as i32;
                    match header.predictor {
                        1 => ra,
                        2 => rb,
                        3 => rc,
                        4 => ra + rb - rc,
                        5 => ra + ((rb - rc) >> 1),
                        6 => rb + ((ra - rc) >> 1),
                        7 => (ra + rb) >> 1,
                        other => bail!("unsupported lossless predictor {other}"),
                    }
                };

                let table = &header.tables[header.table_for_component[component]];
                let category = reader.decode_huffman(table)? as u32;
                let difference = if category == 16 {
                    // T.81 H.1.2.2: SSSS = 16 codes a difference of exactly 32768
                    // with no additional bits.
                    32768
                } else {
                    ensure!(category <= 15, "invalid difference category {category}");
                    let raw = reader.get_bits(category) as i32;
                    extend(raw, category)
                };

                // Wrapping keeps the decoder on the same modular arithmetic the
                // encoder used; the mask then restores the sample precision.
                let value = prediction.wrapping_add(difference) & sample_mask;
                current[index] = (value << header.point_transform) as u16;

                if component + 1 == components {
                    // Only clear these once the whole first MCU has been coded.
                    interval_start = false;
                }
            }

            if info.restart_interval > 0 {
                mcus_until_restart -= 1;
            }
        }

        interval_first_line = false;

        // Copy the finished line out, clipped to the destination.
        let destination_row = row_origin + row;
        let row_base = destination_row * destination_stride;
        if row_base >= destination.len() {
            continue;
        }
        let start = row_base + sample_origin;
        if start >= destination.len() {
            continue;
        }
        let row_limit = (row_base + destination_stride).min(destination.len());
        let available = row_limit.saturating_sub(start);
        let length = available.min(samples_per_line);
        destination[start..start + length].copy_from_slice(&current[..length]);
    }

    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extend_matches_the_spec_examples() {
        // Category 1 codes -1 and 1.
        assert_eq!(extend(0, 1), -1);
        assert_eq!(extend(1, 1), 1);
        // Category 3 spans -7..=-4 and 4..=7.
        assert_eq!(extend(0, 3), -7);
        assert_eq!(extend(3, 3), -4);
        assert_eq!(extend(4, 3), 4);
        assert_eq!(extend(7, 3), 7);
        assert_eq!(extend(0, 0), 0);
    }

    #[test]
    fn huffman_table_decodes_canonical_codes() {
        // Three symbols: one 1-bit code, two 2-bit codes -> 0, 10, 11.
        let counts = [1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let table = HuffTable::new(&counts, vec![7, 8, 9]).unwrap();

        // 0 -> 7, 10 -> 8, 11 -> 9. Pack as bits then read them back.
        let mut reader = BitReader::new(&[0b0101_1000]);
        assert_eq!(reader.decode_huffman(&table).unwrap(), 7);
        assert_eq!(reader.decode_huffman(&table).unwrap(), 8);
        assert_eq!(reader.decode_huffman(&table).unwrap(), 9);
    }

    #[test]
    fn bit_reader_unstuffs_ff00() {
        let mut reader = BitReader::new(&[0xff, 0x00, 0xa5]);
        assert_eq!(reader.get_bits(8), 0xff);
        assert_eq!(reader.get_bits(8), 0xa5);
    }

    #[test]
    fn bit_reader_stops_at_a_marker_and_pads_with_zeroes() {
        // 0xFF 0xD0 is a restart marker, not stuffed data.
        let mut reader = BitReader::new(&[0x5a, 0xff, 0xd0, 0x11]);
        assert_eq!(reader.get_bits(8), 0x5a);
        assert_eq!(
            reader.get_bits(8),
            0x00,
            "marker must not be consumed as data"
        );
        assert_eq!(reader.data[reader.position], 0xff);
    }

    #[test]
    fn restart_skips_past_the_marker() {
        let mut reader = BitReader::new(&[0x5a, 0xff, 0xd3, 0x11, 0x22]);
        assert_eq!(reader.get_bits(8), 0x5a);
        reader.restart().unwrap();
        assert_eq!(reader.get_bits(8), 0x11);
    }

    #[test]
    fn restart_reports_a_missing_marker() {
        let mut reader = BitReader::new(&[0x01, 0x02, 0x03]);
        assert!(reader.restart().is_err());
    }

    /// Build a minimal 1-component SOF3 stream: 4x2 samples, 8-bit, predictor 1,
    /// with a restart marker after every row.
    ///
    /// The Huffman table assigns category 0 the code `0`, category 1 `10` and
    /// category 2 `110`, so the entropy data can be written out by hand.
    fn synthetic_restart_stream() -> Vec<u8> {
        let mut stream = vec![0xff, MARKER_SOI];

        // SOF3: precision 8, 2 rows, 4 columns, 1 component at 1x1.
        stream.extend_from_slice(&[0xff, MARKER_SOF3, 0x00, 0x0b, 8, 0x00, 0x02, 0x00, 0x04, 1]);
        stream.extend_from_slice(&[0x00, 0x11, 0x00]);

        // DHT slot 0: one code each of length 1, 2 and 3 -> categories 0, 1, 2.
        stream.extend_from_slice(&[0xff, MARKER_DHT, 0x00, 0x16, 0x00]);
        stream.extend_from_slice(&[1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        stream.extend_from_slice(&[0, 1, 2]);

        // DRI: restart every 4 MCUs, i.e. every row.
        stream.extend_from_slice(&[0xff, MARKER_DRI, 0x00, 0x04, 0x00, 0x04]);

        // SOS: 1 component, table 0, predictor 1, point transform 0.
        stream.extend_from_slice(&[0xff, MARKER_SOS, 0x00, 0x08, 1, 0x00, 0x00, 1, 0x00, 0x00]);

        // Row 0 diffs (+1, +1, -1, -1), each category 1 ("10") plus one bit.
        // 101 101 100 100 -> 10110110 0100 + padding.
        stream.extend_from_slice(&[0b1011_0110, 0b0100_1111]);
        stream.extend_from_slice(&[0xff, 0xd0]);

        // Row 1 diffs (-1, 0, +1, 0): 100 0 101 0 -> 10001010.
        stream.push(0b1000_1010);
        stream.extend_from_slice(&[0xff, MARKER_EOI]);

        stream
    }

    #[test]
    fn restart_intervals_decode_to_the_expected_samples() {
        let stream = synthetic_restart_stream();
        let info = read_stream_info(&stream).unwrap();
        assert_eq!(info.width, 4);
        assert_eq!(info.height, 2);
        assert_eq!(info.components, 1);
        assert_eq!(info.restart_interval, 4);
        assert!(uses_restart_intervals(&stream));

        let mut destination = vec![0u16; 8];
        decode_into(&stream, &mut destination, 4, 0, 0).unwrap();

        // Row 0 starts the scan at the default prediction 2^(8-1) = 128, then
        // predicts from the sample to the left. Row 0: 129, 130, 129, 128.
        // The restart resets prediction to 128 again, so row 1 is not predicted
        // from the row above: 127, 127, 128, 128.
        assert_eq!(destination, vec![129, 130, 129, 128, 127, 127, 128, 128]);
    }

    /// The whole point of the module: ignoring restart markers corrupts the
    /// image instead of failing, so confirm a restart-free read diverges.
    #[test]
    fn ignoring_the_restart_marker_would_produce_different_samples() {
        let stream = synthetic_restart_stream();
        let mut correct = vec![0u16; 8];
        decode_into(&stream, &mut correct, 4, 0, 0).unwrap();

        // Strip the DRI segment so the decoder never resynchronizes.
        let mut without_dri = stream.clone();
        let dri_at = without_dri
            .windows(2)
            .position(|pair| pair == [0xff, MARKER_DRI])
            .unwrap();
        without_dri.drain(dri_at..dri_at + 6);

        let mut naive = vec![0u16; 8];
        // Without restart handling the reader runs into the marker: it either
        // desynchronizes outright or decodes different samples. Either way it
        // must not reproduce the correct image.
        match decode_into(&without_dri, &mut naive, 4, 0, 0) {
            Ok(_) => assert_ne!(
                correct, naive,
                "restart handling must change the decoded result"
            ),
            Err(error) => assert!(
                error.to_string().contains("desynchronized"),
                "unexpected failure: {error}"
            ),
        }
    }

    #[test]
    fn decode_into_clips_at_the_destination_edges() {
        let stream = synthetic_restart_stream();
        // Destination is one row shorter and two samples narrower than the stream.
        let mut destination = vec![0u16; 2];
        decode_into(&stream, &mut destination, 2, 0, 0).unwrap();
        assert_eq!(destination, vec![129, 130]);
    }

    #[test]
    fn lossy_streams_are_rejected_rather_than_misread() {
        // SOI followed by a DQT segment.
        let stream = [0xff, MARKER_SOI, 0xff, MARKER_DQT, 0x00, 0x03, 0x00];
        let error = match parse_header(&stream) {
            Ok(_) => panic!("a lossy stream must not parse as lossless"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("lossy"), "unexpected error: {error}");
    }
}
