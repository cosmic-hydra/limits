//! `xodfe` command-line tool.

use std::process::ExitCode;
use xodfe::error::{fail, Result};
use xodfe::llama::{generate, Sampler, Tokenizer, Transformer};
use xodfe::{llama2c, quant, safetensors, DType, Model, Writer};

const USAGE: &str = "usage:
  xodfe info FILE
  xodfe verify FILE
  xodfe convert IN OUT [--tokenizer tokenizer.bin]   (IN: .safetensors or llama2.c .bin)
  xodfe export FILE OUT.safetensors
  xodfe quantize IN OUT --to q8_0|q4_0   (2-D float tensors with rows % 32 == 0)
  xodfe run FILE --prompt \"...\" [-n 256] [--temp 0.0] [--topp 0.9] [--seed N] [--threads N]
  xodfe bench FILE [-n 256] [--threads N] [--ref OTHER.xodfe]   (or FILE.safetensors: header parse)";

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn info(path: &str) -> Result<()> {
    use std::fmt::Write;
    let mut o = String::new();
    let m = Model::open(path, false)?;
    let h = &m.header;
    let _ = writeln!(
        o,
        "{path}: XODFe v1, {} bytes, data section at {}",
        m.file_len(),
        h.data_start
    );
    let _ = writeln!(o, "metadata ({}):", h.meta.len());
    for (k, v) in &h.meta {
        let _ = writeln!(o, "  {k:<32} {}", v.summary());
    }
    let _ = writeln!(o, "tensors ({}):", h.tensors.len());
    let _ = writeln!(
        o,
        "  {:<40} {:<5} {:<18} {:>12} {:>12}",
        "name", "dtype", "shape", "offset", "bytes"
    );
    let mut total = 0;
    for t in &h.tensors {
        let shape = format!("{:?}", t.dims);
        let _ = writeln!(
            o,
            "  {:<40} {:<5} {:<18} {:>12} {:>12}",
            t.name,
            t.dtype.name(),
            shape,
            t.offset,
            t.nbytes
        );
        total += t.numel();
    }
    let _ = writeln!(o, "total parameters: {total}");
    // Ignore EPIPE (e.g. `xodfe info f | head`).
    let _ = std::io::Write::write_all(&mut std::io::stdout(), o.as_bytes());
    Ok(())
}

fn run(args: &[String]) -> Result<()> {
    let a = |i: usize| {
        args.get(i)
            .map(String::as_str)
            .map_or_else(|| fail(USAGE), Ok)
    };
    match a(0)? {
        "info" => info(a(1)?),
        "verify" => {
            let m = Model::open(a(1)?, true)?;
            println!(
                "OK: {} metadata entries, {} tensors, all checksums valid",
                m.header.meta.len(),
                m.tensors().len()
            );
            Ok(())
        }
        "convert" => {
            let (inp, out) = (a(1)?, a(2)?);
            let bytes = std::fs::read(inp)?;
            let w = if inp.ends_with(".safetensors") {
                safetensors::import(&bytes)?
            } else {
                let tok = flag(args, "--tokenizer").map(std::fs::read).transpose()?;
                llama2c::import(&bytes, tok.as_deref())?
            };
            w.write(out)?;
            Model::open(out, true)?; // re-validate what we wrote
            println!("wrote {out}");
            Ok(())
        }
        "export" => {
            let m = Model::open(a(1)?, true)?;
            std::fs::write(a(2)?, safetensors::export(&m)?)?;
            println!("wrote {}", a(2)?);
            Ok(())
        }
        "quantize" => quantize(a(1)?, a(2)?, flag(args, "--to").unwrap_or("")),
        "run" => run_model(a(1)?, args),
        "bench" => bench(a(1)?, args),
        _ => fail(USAGE),
    }
}

fn quantize(inp: &str, out: &str, to: &str) -> Result<()> {
    let q: fn(&[f32]) -> Vec<u8> = match DType::parse(to) {
        Some(DType::Q8_0) => quant::quantize_q8_0,
        Some(DType::Q4_0) => quant::quantize_q4_0,
        _ => return fail("quantize: --to must be q8_0 or q4_0"),
    };
    let m = Model::open(inp, true)?;
    let mut w = Writer::new();
    for (k, v) in &m.header.meta {
        w.meta(k, v.clone());
    }
    let mut n = 0;
    for t in m.tensors() {
        let float = matches!(t.dtype, DType::F32 | DType::F16 | DType::BF16);
        if float && t.dims.len() == 2 && t.dims[1] % 32 == 0 {
            w.tensor(&t.name, DType::parse(to).unwrap(), &t.dims, q(&m.to_f32(t)))?;
            n += 1;
        } else {
            w.tensor(&t.name, t.dtype, &t.dims, m.data(t).to_vec())?;
        }
    }
    w.meta("general.quantization", xodfe::Value::Str(to.into()));
    w.write(out)?;
    let size = Model::open(out, true)?.file_len();
    println!("wrote {out}: {n} tensors quantized to {to}, {size} bytes");
    Ok(())
}

fn num<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> Result<T> {
    match flag(args, name) {
        None => Ok(default),
        Some(v) => v
            .parse()
            .map_or_else(|_| fail(format!("bad value for {name}: {v:?}")), Ok),
    }
}

/// `-n` (positions to run, prompt included); must be at least 1.
fn steps_arg(args: &[String]) -> Result<usize> {
    match num(args, "-n", 256)? {
        0 => fail("-n must be at least 1"),
        n => Ok(n),
    }
}

fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

fn load(path: &str, threads: usize, ctx: usize) -> Result<(Transformer, Tokenizer)> {
    let m = Model::open(path, false)?;
    let t = Transformer::load(&m, threads, ctx)?;
    let tok = Tokenizer::from_model(&m, t.cfg.vocab_size)?;
    Ok((t, tok))
}

fn run_model(path: &str, args: &[String]) -> Result<()> {
    use std::io::Write;
    let steps = steps_arg(args)?;
    let (mut t, tok) = load(path, num(args, "--threads", default_threads())?, steps)?;
    let seed = num(args, "--seed", 0u64)?;
    let seed = if seed == 0 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64)
    } else {
        seed
    };
    let mut s = Sampler::new(num(args, "--temp", 1.0)?, num(args, "--topp", 0.9)?, seed);
    let prompt = flag(args, "--prompt").unwrap_or("");
    let mut out = std::io::stdout().lock();
    let (n, secs) = generate(&mut t, &tok, &mut s, prompt, steps, |p| {
        let _ = out.write_all(p);
        let _ = out.flush();
    });
    let _ = writeln!(out);
    if n > 1 && secs > 0.0 {
        eprintln!("achieved tok/s: {:.1}", (n - 1) as f64 / secs);
    }
    Ok(())
}

/// Median wall time of `f` over `reps` runs, in milliseconds.
fn time_ms<T>(reps: usize, mut f: impl FnMut() -> Result<T>) -> Result<f64> {
    let mut v = Vec::new();
    for _ in 0..reps {
        let t = std::time::Instant::now();
        std::hint::black_box(f()?);
        v.push(t.elapsed().as_secs_f64() * 1e3);
    }
    v.sort_by(f64::total_cmp);
    Ok(v[reps / 2])
}

/// Greedy token ids for `prompt` (prompt tokens included).
fn greedy(t: &mut Transformer, tok: &Tokenizer, prompt: &str, n: usize) -> Vec<usize> {
    let mut ids = tok.encode(prompt);
    let np = ids.len();
    for pos in 0..n.min(t.cfg.seq_len).saturating_sub(1) {
        let next = Sampler::new(0.0, 0.9, 1).sample(t.forward(ids[pos], pos));
        if pos + 1 >= np {
            ids.push(next);
        }
    }
    ids
}

const PROMPTS: [&str; 3] = ["Once upon a time", "The little dog", "Lily wanted to"];

fn bench(path: &str, args: &[String]) -> Result<()> {
    if path.ends_with(".safetensors") {
        let b = std::fs::read(path)?;
        let n = b
            .get(..8)
            .map_or(0, |h| u64::from_le_bytes(h.try_into().unwrap()));
        let Some(h) = b.get(8..).and_then(|r| r.get(..n as usize)) else {
            return fail("bench: bad safetensors header length");
        };
        let ms = time_ms(21, || xodfe::json::parse(h))?;
        println!("safetensors JSON header parse: {:.3} ms (median of 21)", ms);
        return Ok(());
    }
    let mmap = time_ms(21, || Model::open(path, false))?;
    let read = time_ms(21, || Model::from_vec(std::fs::read(path)?, false))?;
    let m = Model::open(path, false)?;
    let bytes = std::fs::read(path)?;
    let parse = time_ms(21, || xodfe::format::parse(&bytes))?;
    let crc = time_ms(5, || m.verify_checksums())?;
    println!(
        "{} bytes, header {} bytes\nopen+validate: mmap {mmap:.3} ms, read {read:.3} ms; \
         header parse {parse:.3} ms; crc32 all tensors {crc:.2} ms (medians)",
        m.file_len(),
        m.header.data_start
    );
    let n = steps_arg(args)?;
    let max = num(args, "--threads", default_threads())?;
    let mut threads = vec![1];
    if max > 1 {
        threads.push(max);
    }
    for th in threads {
        let t1 = std::time::Instant::now();
        let mut t = Transformer::load(&m, th, n)?;
        let tok = Tokenizer::from_model(&m, t.cfg.vocab_size)?;
        let ld = t1.elapsed().as_secs_f64();
        let mut s = Sampler::new(0.0, 0.9, 1);
        let (steps, secs) = generate(&mut t, &tok, &mut s, PROMPTS[0], n, |_| {});
        println!(
            "threads={th}: load {:.1} ms, {} tokens, {:.1} tok/s (greedy)",
            ld * 1e3,
            steps,
            steps.saturating_sub(1) as f64 / secs.max(f64::MIN_POSITIVE)
        );
    }
    // Greedy agreement with a reference model (e.g. the f32 original).
    if let Some(r) = flag(args, "--ref") {
        let rm = Model::open(r, false)?;
        let (mut a, mut b) = (
            Transformer::load(&m, max, n)?,
            Transformer::load(&rm, max, n)?,
        );
        let tok = Tokenizer::from_model(&m, a.cfg.vocab_size)?;
        for p in PROMPTS {
            let (x, y) = (greedy(&mut a, &tok, p, n), greedy(&mut b, &tok, p, n));
            let np = tok.encode(p).len();
            let same = x
                .iter()
                .zip(&y)
                .skip(np)
                .take_while(|(i, j)| i == j)
                .count();
            let total = x.len() - np;
            println!("{p:?}: first {same}/{total} generated tokens identical to {r}");
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xodfe: {e}");
            ExitCode::FAILURE
        }
    }
}
