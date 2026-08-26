//! Golden fixtures for the C5 illuminant estimator.
//!
//! Three layers, so a failure says which half broke:
//!
//! 1. the histogram builder, against the stack the Python port produced from
//!    the same proxy pixels;
//! 2. the post-processing, against the illuminant the port computed from the
//!    ONNX filter and bias;
//! 3. the whole path, ONNX session included.
//!
//! Layer 3 is the only one that needs `models/artifacts/`, which is gitignored
//! — it reports and returns rather than failing when the artifact is absent, so
//! a fresh clone still gets layers 1 and 2.
//!
//! Regenerate the fixtures with
//! `.venv-train/bin/python tools/scene_models/c5_port.py --stage fixtures`;
//! `tests/fixtures/c5/README.md` documents every formula they pin.

use raw_autotune::illuminant::{self, CameraProxy, HIST_SIZE, PROXY_HEIGHT, PROXY_WIDTH};
use std::path::{Path, PathBuf};

const PLANE: usize = HIST_SIZE * HIST_SIZE;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/c5")
        .join(name)
}

fn read_f32(path: &Path, expected: usize) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert_eq!(
        bytes.len(),
        expected * 4,
        "{} holds {} bytes, expected {} floats",
        path.display(),
        bytes.len(),
        expected
    );
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

/// The fixture proxy, HWC f32, exactly as `ops.resize_image` produced it.
fn golden_proxy() -> CameraProxy {
    let flat = read_f32(&fixture("image.bin"), PROXY_WIDTH * PROXY_HEIGHT * 3);
    CameraProxy {
        width: PROXY_WIDTH,
        height: PROXY_HEIGHT,
        rgb: flat
            .chunks_exact(3)
            .map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect(),
        source_width: PROXY_WIDTH,
        source_height: PROXY_HEIGHT,
    }
}

fn expected_illuminant() -> [f32; 3] {
    let text = std::fs::read_to_string(fixture("expected.json")).expect("read expected.json");
    // A three-float array out of a flat fixture: pulling in a JSON parser for
    // one known key would be the larger dependency.
    let start = text.find("\"illuminant_rgb\"").expect("illuminant_rgb key");
    let open = text[start..].find('[').expect("illuminant_rgb array") + start;
    let close = text[open..].find(']').expect("illuminant_rgb array end") + open;
    let values: Vec<f32> = text[open + 1..close]
        .split(',')
        .map(|value| value.trim().parse().expect("illuminant_rgb float"))
        .collect();
    assert_eq!(values.len(), 3);
    [values[0], values[1], values[2]]
}

fn angular_error_deg(a: [f32; 3], b: [f32; 3]) -> f64 {
    let dot = (0..3).map(|i| a[i] as f64 * b[i] as f64).sum::<f64>();
    let norm = |v: [f32; 3]| (0..3).map(|i| (v[i] as f64).powi(2)).sum::<f64>().sqrt();
    (dot / (norm(a) * norm(b)))
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

#[test]
fn histogram_stack_matches_the_python_port() {
    let expected = read_f32(&fixture("hist_stack.bin"), 4 * PLANE);
    let stack = illuminant::histogram_stack(&golden_proxy());
    assert_eq!(stack.len(), expected.len());

    // Per plane, because the four are not equally reproducible from the
    // fixture. `hist_stack.bin` was built from the float64 image; `image.bin`
    // is that image rounded to float32 for storage. The chroma histogram and
    // the coordinate planes are insensitive to that rounding, but the edge
    // histogram is built from differences between adjacent pixels — values
    // around 1e-3 — where float32 storage costs several digits of relative
    // precision before the log-chroma ratio is even taken. A transcription of
    // the upstream formulas in numpy, run on `image.bin`, lands on the same
    // 3.04e-5, so this bound is the fixture's resolution and not a divergence.
    const TOLERANCE: [f32; 4] = [1.0e-6, 1.0e-4, 1.0e-7, 1.0e-7];
    for plane in 0..4 {
        let mut worst = 0.0_f32;
        let mut worst_bin = 0;
        for bin in 0..PLANE {
            let index = plane * PLANE + bin;
            let difference = (stack[index] - expected[index]).abs();
            if difference > worst {
                worst = difference;
                worst_bin = bin;
            }
        }
        eprintln!("C5 histogram plane {plane} max |diff| = {worst:.3e}");
        assert!(
            worst < TOLERANCE[plane],
            "plane {plane} diverged by {worst} at bin {worst_bin} (got {}, want {})",
            stack[plane * PLANE + worst_bin],
            expected[plane * PLANE + worst_bin]
        );
    }
}

#[test]
fn apply_ccc_matches_the_python_port() {
    let stack = read_f32(&fixture("hist_stack.bin"), 4 * PLANE);
    let filter = read_f32(&fixture("ccc_filter.bin"), 2 * PLANE);
    let bias = read_f32(&fixture("ccc_bias.bin"), PLANE);

    let rgb = illuminant::apply_ccc(&stack, &filter, &bias).expect("apply_ccc");
    let expected = expected_illuminant();
    let error = angular_error_deg(rgb, expected);
    eprintln!("C5 apply_ccc angular error = {error:.6} deg (got {rgb:?}, want {expected:?})");
    assert!(error < 0.01, "apply_ccc diverged by {error} degrees");

    let norm = (0..3).map(|i| (rgb[i] as f64).powi(2)).sum::<f64>().sqrt();
    assert!((norm - 1.0).abs() < 1.0e-5, "illuminant is not unit norm");
}

#[test]
fn onnx_end_to_end_matches_the_python_port() {
    let model = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("models/artifacts")
        .join(illuminant::MODEL_FILE);
    if !model.exists() {
        eprintln!(
            "SKIP  {} is absent (models/artifacts is gitignored); \
             re-export with tools/scene_models/c5_port.py --stage export",
            model.display()
        );
        return;
    }

    let stack = read_f32(&fixture("hist_stack.bin"), 4 * PLANE);
    let session = lege_gpu::vision::OnnxSession::from_path(&model).expect("load C5 graph");
    let input = lege_gpu::vision::Tensor::new(vec![1, 4, HIST_SIZE, HIST_SIZE], stack.clone())
        .expect("build C5 input tensor");
    // CPU deliberately: the in-graph m=7 duplication produces rank-5
    // intermediates the wgpu compiler has no coverage for, and a wrong answer
    // there would look like a plausible illuminant rather than an error.
    let outputs = session.run_cpu(&input).expect("run C5 on the CPU");

    let filter = outputs.get("ccc_filter").expect("ccc_filter output");
    let bias = outputs.get("ccc_bias").expect("ccc_bias output");
    assert_eq!(filter.shape, vec![1, 2, HIST_SIZE, HIST_SIZE]);
    assert_eq!(bias.shape, vec![1, HIST_SIZE, HIST_SIZE]);

    let rgb = illuminant::apply_ccc(&stack, &filter.data, &bias.data).expect("apply_ccc");
    let expected = expected_illuminant();
    let error = angular_error_deg(rgb, expected);
    eprintln!("C5 end-to-end angular error = {error:.6} deg (got {rgb:?}, want {expected:?})");
    assert!(error < 0.01, "end-to-end diverged by {error} degrees");

    // And again from the histogram Rust builds itself, which is what a real
    // frame goes through. This is the only place the float32 rounding of
    // `image.bin` (see the histogram test) can actually cost anything, so the
    // bound says what it costs: far below the ~2 degrees that separates a good
    // illuminant estimator from a mediocre one.
    let built = illuminant::histogram_stack(&golden_proxy());
    let input = lege_gpu::vision::Tensor::new(vec![1, 4, HIST_SIZE, HIST_SIZE], built.clone())
        .expect("build C5 input tensor");
    let outputs = session.run_cpu(&input).expect("run C5 on the CPU");
    let rgb = illuminant::apply_ccc(
        &built,
        &outputs["ccc_filter"].data,
        &outputs["ccc_bias"].data,
    )
    .expect("apply_ccc");
    let error = angular_error_deg(rgb, expected);
    eprintln!("C5 end-to-end from the Rust histogram = {error:.6} deg");
    assert!(error < 0.05, "Rust histogram cost {error} degrees");
}

/// Not a correctness test — a budget. The estimator runs per frame inside a
/// `--jobs N` batch, so a regression that made the histogram or the FFT
/// quadratic would show up here rather than as a slow corpus run.
#[test]
fn histogram_and_post_processing_stay_cheap() {
    let proxy = golden_proxy();
    let filter = read_f32(&fixture("ccc_filter.bin"), 2 * PLANE);
    let bias = read_f32(&fixture("ccc_bias.bin"), PLANE);

    let started = std::time::Instant::now();
    for _ in 0..10 {
        std::hint::black_box(illuminant::histogram_stack(&proxy));
    }
    let histogram = started.elapsed() / 10;

    let stack = illuminant::histogram_stack(&proxy);
    let started = std::time::Instant::now();
    for _ in 0..100 {
        std::hint::black_box(illuminant::apply_ccc(&stack, &filter, &bias).unwrap());
    }
    let post = started.elapsed() / 100;

    eprintln!("C5 histogram {histogram:?}/frame, apply_ccc {post:?}/frame");
    assert!(
        histogram < std::time::Duration::from_millis(200),
        "histogram took {histogram:?}"
    );
    assert!(
        post < std::time::Duration::from_millis(20),
        "apply_ccc took {post:?}"
    );
}
