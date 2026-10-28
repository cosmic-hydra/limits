//! Greedy-decoding parity with karpathy/llama2.c on stories15M.
//!
//! Needs `models/stories15M.bin` + `models/tokenizer.bin` (never committed).
//! Reference, in order of preference:
//!   1. a compiled llama2.c `run` binary: `$XODFE_LLAMA2C_RUN`
//!   2. `tools/ref_llama2c.py` (independent numpy port of run.c), if python3+numpy exist.
//!
//! Skips (printing why) when the model files or every reference are absent.

use std::path::{Path, PathBuf};
use std::process::Command;
use xodfe::llama::{generate, Sampler, Tokenizer, Transformer};
use xodfe::{llama2c, Model};

const PROMPTS: [&str; 3] = ["Once upon a time", "The little dog", "Lily wanted to"];
const STEPS: usize = 200;

fn xodfe_text(m: &Model, prompt: &str) -> String {
    let mut t = Transformer::load(m, 4, STEPS).unwrap();
    let tok = Tokenizer::from_model(m, t.cfg.vocab_size).unwrap();
    let mut out = Vec::new();
    generate(
        &mut t,
        &tok,
        &mut Sampler::new(0.0, 0.9, 1),
        prompt,
        STEPS,
        |p| out.extend_from_slice(p),
    );
    String::from_utf8(out).unwrap()
}

fn reference(dir: &Path, prompt: &str) -> Option<(String, String)> {
    let (bin, tok) = (dir.join("stories15M.bin"), dir.join("tokenizer.bin"));
    let out = if let Some(run) = std::env::var_os("XODFE_LLAMA2C_RUN") {
        let mut c = Command::new(&run);
        c.arg(&bin).arg("-z").arg(&tok);
        c.args(["-t", "0.0", "-n", &STEPS.to_string(), "-i", prompt]);
        (c.output().ok()?, "llama2.c run")
    } else {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/ref_llama2c.py");
        let mut c = Command::new("python3");
        c.arg("-I").arg(script).arg(&bin).arg(&tok).arg(prompt);
        c.arg(STEPS.to_string());
        (c.output().ok()?, "numpy ref_llama2c.py")
    };
    if !out.0.status.success() {
        eprintln!(
            "reference failed: {}",
            String::from_utf8_lossy(&out.0.stderr)
        );
        return None;
    }
    let s = String::from_utf8(out.0.stdout).ok()?;
    Some((s.trim_end_matches('\n').to_string(), out.1.to_string()))
}

#[test]
fn greedy_parity_with_llama2c() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models");
    let (bin, tok) = (dir.join("stories15M.bin"), dir.join("tokenizer.bin"));
    let (Ok(ckpt), Ok(tk)) = (std::fs::read(&bin), std::fs::read(&tok)) else {
        eprintln!("SKIP: {} / {} not found", bin.display(), tok.display());
        return;
    };
    let w = llama2c::import(&ckpt, Some(&tk)).unwrap();
    let m = Model::from_vec(w.to_bytes(), true).unwrap();
    for p in PROMPTS {
        let Some((want, by)) = reference(&dir, p) else {
            eprintln!("SKIP: no reference available (set XODFE_LLAMA2C_RUN or install numpy)");
            return;
        };
        let got = xodfe_text(&m, p);
        assert_eq!(got, want, "prompt {p:?} differs from {by}");
        eprintln!("prompt {p:?}: identical to {by} ({} bytes)", got.len());
    }
}
