//! Load a prepared `scene_image` ONNX graph through lege-gpu and run one
//! inference pass. This is the model-pack acceptance probe, not a renderer.
//!
//! ```text
//! cargo run --release --example probe-scene-model -- \
//!     models/artifacts/yunet.prepared.onnx
//! ```

fn main() -> anyhow::Result<()> {
    use anyhow::Context;
    use lege_gpu::vision::{OnnxSession, Tensor};
    use std::path::PathBuf;

    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("models/artifacts/yunet.prepared.onnx"));
    if !path.exists() {
        anyhow::bail!(
            "prepared model {} is missing; run tools/scene_models/pack.py",
            path.display()
        );
    }

    let session = OnnxSession::from_path(&path)
        .with_context(|| format!("failed to load {}", path.display()))?;
    println!("input: {}", session.input_name());
    println!("outputs: {}", session.output_names().join(", "));

    let shape = session
        .input_shape()
        .filter(|dims| dims.len() == 4 && dims.iter().all(|d| *d > 0))
        .map(|dims| {
            dims.iter()
                .map(|d| usize::try_from(*d).expect("positive dim"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![1, 3, 512, 512]);
    let elements = shape.iter().product();
    println!("input shape: {shape:?}");
    let input = Tensor::new(shape, vec![0.0f32; elements])?;
    let outputs = session
        .run_gpu(&input)
        .context("lege-gpu wgpu inference failed")?;
    let mut names: Vec<_> = outputs.keys().cloned().collect();
    names.sort();
    for name in names {
        let tensor = &outputs[&name];
        println!("  {name}: {:?}", tensor.shape);
    }
    Ok(())
}
