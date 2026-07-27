//! Debug helper: dump the DNG tags that decide the sample domain.
//!
//! cargo run --release --example probe-tiff -- <file.dng>

use rawler::formats::tiff::reader::TiffReader;
use rawler::formats::tiff::{GenericTiffReader, Value};
use std::fs::File;
use std::io::BufReader;

// DNG tag ids (see DNG 1.7 spec).
const LINEARIZATION_TABLE: u16 = 50712;
const BLACK_LEVEL: u16 = 50714;
const WHITE_LEVEL: u16 = 50717;
const BLACK_LEVEL_REPEAT_DIM: u16 = 50713;
const PHOTOMETRIC: u16 = 262;
const BITS_PER_SAMPLE: u16 = 258;
const SAMPLES_PER_PIXEL: u16 = 277;
const SUB_IFDS: u16 = 330;
const COMPRESSION: u16 = 259;
// Opcode lists describe corrections a reader is expected to apply.
const OPCODE_LIST_1: u16 = 51008;
const OPCODE_LIST_2: u16 = 51009;
const OPCODE_LIST_3: u16 = 51022;
const BASELINE_EXPOSURE: u16 = 50730;

fn summarize(name: &str, value: &Value) {
    match value {
        Value::Short(values) => {
            if values.len() > 8 {
                println!(
                    "  {name}: Short count={} first={:?} last={:?} min={:?} max={:?}",
                    values.len(),
                    &values[..4],
                    &values[values.len() - 4..],
                    values.iter().min(),
                    values.iter().max()
                );
            } else {
                println!("  {name}: Short {values:?}");
            }
        }
        Value::Long(values) => println!("  {name}: Long {values:?}"),
        Value::Rational(values) => println!(
            "  {name}: Rational {:?}",
            values.iter().map(|r| r.as_f32()).collect::<Vec<_>>()
        ),
        other => println!("  {name}: {other:?}"),
    }
}

fn main() -> anyhow::Result<()> {
    for path in std::env::args().skip(1) {
        println!("--- {path}");
        let mut file = BufReader::new(File::open(&path)?);
        let tiff = GenericTiffReader::new(&mut file, 0, 0, None, &[SUB_IFDS])?;

        let tags = [
            (PHOTOMETRIC, "PhotometricInterpretation"),
            (BITS_PER_SAMPLE, "BitsPerSample"),
            (COMPRESSION, "Compression"),
            (SAMPLES_PER_PIXEL, "SamplesPerPixel"),
            (BLACK_LEVEL, "BlackLevel"),
            (BLACK_LEVEL_REPEAT_DIM, "BlackLevelRepeatDim"),
            (WHITE_LEVEL, "WhiteLevel"),
            (LINEARIZATION_TABLE, "LinearizationTable"),
            (OPCODE_LIST_1, "OpcodeList1 (pre-demosaic)"),
            (OPCODE_LIST_2, "OpcodeList2 (post-demosaic, linear)"),
            (OPCODE_LIST_3, "OpcodeList3 (post-demosaic, gamma)"),
            (BASELINE_EXPOSURE, "BaselineExposure"),
        ];

        for ifd in tiff.root_ifd().find_ifds_with_tag(PHOTOMETRIC) {
            let photometric = ifd.get_entry(PHOTOMETRIC).map(|e| e.force_u32(0));
            println!(" IFD (photometric={photometric:?}):");
            for (tag, name) in tags {
                if let Some(entry) = ifd.get_entry(tag) {
                    summarize(name, &entry.value);
                }
            }
        }
    }
    Ok(())
}
