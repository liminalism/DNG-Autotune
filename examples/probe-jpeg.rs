//! Debug helper: list the JPEG markers in the first strip/tile of the raw IFD.
//!
//! cargo run --release --example probe-jpeg -- <file.dng>

use rawler::formats::tiff::GenericTiffReader;
use rawler::formats::tiff::reader::TiffReader;
use std::fs;

const PHOTOMETRIC: u16 = 262;
const STRIP_OFFSETS: u16 = 273;
const STRIP_BYTE_COUNTS: u16 = 279;
const TILE_OFFSETS: u16 = 324;
const TILE_BYTE_COUNTS: u16 = 325;
const SUB_IFDS: u16 = 330;

fn marker_name(marker: u8) -> &'static str {
    match marker {
        0xc0 => "SOF0 (baseline DCT)",
        0xc1 => "SOF1 (extended sequential DCT)",
        0xc2 => "SOF2 (progressive DCT)",
        0xc3 => "SOF3 (LOSSLESS)",
        0xc4 => "DHT",
        0xc9 => "SOF9 (arithmetic sequential)",
        0xcb => "SOF11 (arithmetic lossless)",
        0xd8 => "SOI",
        0xd9 => "EOI",
        0xda => "SOS",
        0xdb => "DQT",
        0xdd => "DRI",
        0xe0..=0xef => "APPn",
        0xfe => "COM",
        _ => "other",
    }
}

fn main() -> anyhow::Result<()> {
    for path in std::env::args().skip(1) {
        println!("--- {path}");
        let bytes = fs::read(&path)?;
        let mut cursor = std::io::Cursor::new(&bytes);
        let tiff = GenericTiffReader::new(&mut cursor, 0, 0, None, &[SUB_IFDS])?;

        for ifd in tiff.root_ifd().find_ifds_with_tag(PHOTOMETRIC) {
            let photometric = ifd
                .get_entry(PHOTOMETRIC)
                .map(|e| e.force_u32(0))
                .unwrap_or(0);
            // Only the raw IFD matters here (LinearRaw or CFA).
            if photometric != 34892 && photometric != 32803 {
                continue;
            }

            let (offsets, counts) = match ifd.get_entry(STRIP_OFFSETS) {
                Some(entry) => (entry, ifd.get_entry(STRIP_BYTE_COUNTS)),
                None => match ifd.get_entry(TILE_OFFSETS) {
                    Some(entry) => (entry, ifd.get_entry(TILE_BYTE_COUNTS)),
                    None => continue,
                },
            };

            let count = offsets.count() as usize;
            let first = offsets.value.force_usize(0);
            let length = counts.map(|c| c.value.force_usize(0)).unwrap_or(0);
            println!(
                "  photometric={photometric} chunks={count} first_offset={first} first_len={length}"
            );

            let chunk = &bytes[first..(first + length).min(bytes.len())];
            println!("  first bytes: {:02x?}", &chunk[..16.min(chunk.len())]);

            // Walk JPEG markers until SOS (entropy data follows).
            let mut i = 0usize;
            while i + 1 < chunk.len() {
                if chunk[i] != 0xff {
                    i += 1;
                    continue;
                }
                let marker = chunk[i + 1];
                if marker == 0xff || marker == 0x00 {
                    i += 1;
                    continue;
                }
                print!("    @{i} 0xff{marker:02x} {}", marker_name(marker));
                if marker == 0xd8 {
                    println!();
                    i += 2;
                    continue;
                }
                if i + 3 >= chunk.len() {
                    println!();
                    break;
                }
                let seg_len = ((chunk[i + 2] as usize) << 8) | chunk[i + 3] as usize;
                if (0xc0..=0xcf).contains(&marker)
                    && marker != 0xc4
                    && marker != 0xc8
                    && marker != 0xcc
                {
                    let precision = chunk[i + 4];
                    let height = ((chunk[i + 5] as usize) << 8) | chunk[i + 6] as usize;
                    let width = ((chunk[i + 7] as usize) << 8) | chunk[i + 8] as usize;
                    let components = chunk[i + 9];
                    print!(" precision={precision} {width}x{height} components={components}");
                }
                println!(" len={seg_len}");
                if marker == 0xda {
                    break;
                }
                i += 2 + seg_len;
            }
        }
    }
    Ok(())
}
