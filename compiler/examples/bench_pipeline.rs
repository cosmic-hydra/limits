//! Dependency-free, reproducible phase timings. Run in release mode.
use limits::{codegen, interp::Interpreter, lexer, parser, resolve, vm::Vm};
use std::time::Instant;

fn median(values: &mut [u128]) -> u128 {
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        values[middle - 1] + (values[middle] - values[middle - 1]) / 2
    } else {
        values[middle]
    }
}
fn benchmark(label: &str, source: &str, samples: usize) {
    let mut timings: [Vec<u128>; 6] = std::array::from_fn(|_| Vec::with_capacity(samples));
    let mut tokens_count = 0;
    let mut instruction_count = 0;
    let mut constant_count = 0;
    for sample in 0..=samples {
        let start = Instant::now();
        let tokens = lexer::lex(source).unwrap();
        let lex = start.elapsed().as_nanos();
        tokens_count = tokens.len();
        let start = Instant::now();
        let ast = parser::parse(tokens).unwrap();
        let parse = start.elapsed().as_nanos();
        let start = Instant::now();
        assert!(resolve::check(&ast).is_empty());
        let check = start.elapsed().as_nanos();
        let start = Instant::now();
        let compiled = codegen::compile(&ast).unwrap();
        let compile = start.elapsed().as_nanos();
        instruction_count = compiled.chunk().instruction_count();
        constant_count = compiled.chunk().constant_count();
        let mut tree_out = Vec::new();
        let mut vm_out = Vec::new();
        let mut tree_time = 0;
        let mut vm_time = 0;
        // Alternate engine order; every sample uses fresh runtimes and output.
        for vm in if sample % 2 == 0 {
            [true, false]
        } else {
            [false, true]
        } {
            let start = Instant::now();
            if vm {
                Vm::new(&mut vm_out).run(compiled.clone()).unwrap();
                vm_time = start.elapsed().as_nanos();
            } else {
                Interpreter::new(&mut tree_out).run(&ast).unwrap();
                tree_time = start.elapsed().as_nanos();
            }
        }
        assert_eq!(tree_out, vm_out);
        if sample != 0 {
            for (values, value) in timings
                .iter_mut()
                .zip([lex, parse, check, compile, tree_time, vm_time])
            {
                values.push(value);
            }
        }
    }
    let [lex, parse, check, compile, tree, vm] = timings.map(|mut values| median(&mut values));
    let loc = source
        .lines()
        .filter(|line| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with('#')
        })
        .count();
    println!("{{\"workload\":\"{label}\",\"samples\":{samples},\"source_bytes\":{},\"nonblank_noncomment_loc\":{loc},\"tokens\":{tokens_count},\"top_level_instructions\":{instruction_count},\"top_level_constants\":{constant_count},\"median_ns\":{{\"lex\":{lex},\"parse\":{parse},\"check\":{check},\"compile\":{compile},\"tree_execute\":{tree},\"vm_execute\":{vm}}}}}",source.len());
}
fn main() {
    let mut samples = 9usize;
    let mut width = 8000usize;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&n| n > 0);
        match (flag.as_str(), value) {
            ("--samples", Some(n)) if n <= 1000 => samples = n,
            ("--width", Some(n)) if n <= 100_000 => width = n,
            _ => {
                eprintln!("usage: bench_pipeline [--samples 1..1000] [--width 1..100000]");
                std::process::exit(2);
            }
        }
    }
    let mut wide = String::new();
    for i in 0..width {
        wide.push_str(&format!("let v{i}={i};\n"));
    }
    for i in 0..width {
        wide.push_str(&format!("v{i}=v{i}+1;\n"));
    }
    wide.push_str(&format!("print(v0,v{});\n", width - 1));
    benchmark(
        "agent_report_manual",
        include_str!("../../bench/agent_compact/report_manual.lim"),
        samples,
    );
    benchmark(
        "agent_report_helpers",
        include_str!("../../bench/agent_compact/report_helpers.lim"),
        samples,
    );
    benchmark(
        "agent_actions_original",
        include_str!("../../examples/agent_task.lim"),
        samples,
    );
    benchmark(
        "agent_actions_compact",
        include_str!("../../bench/agent_compact/actions_compact.lim"),
        samples,
    );
    benchmark("wide_scope", &wide, samples);
    benchmark(
        "fib25",
        include_str!("../../examples/bench_fib.lim"),
        samples,
    );
}
