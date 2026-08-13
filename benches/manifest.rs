use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use knight_build::build::{elide_middle, filter_msvc_output};
use knight_build::dyndep::parse_dyndep;
use knight_build::parse_manifest;
use std::collections::BTreeSet;
use std::path::Path;

fn generated_manifest(edges: usize) -> String {
    let mut source = String::with_capacity(edges * 80);
    source.push_str("rule cc\n  command = cc -c $in -o $out\n");
    for index in 0..edges {
        source.push_str(&format!(
            "build obj/{index}.o: cc src/{index}.c | include/common.h\n"
        ));
    }
    source.push_str("build all: phony");
    for index in 0..edges {
        source.push_str(&format!(" obj/{index}.o"));
    }
    source.push_str("\ndefault all\n");
    source
}

fn parse_benchmark(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("manifest_parse");
    for edges in [100usize, 1_000, 10_000] {
        let source = generated_manifest(edges);
        group.throughput(Throughput::Bytes(source.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(edges),
            &source,
            |bench, source| {
                bench.iter(|| parse_manifest(source, "build.ninja").unwrap());
            },
        );
    }
    group.finish();
}

fn generated_dyndep(edges: usize) -> String {
    let mut source = String::with_capacity(edges * 70);
    source.push_str("ninja_dyndep_version = 1\n");
    for index in 0..edges {
        source.push_str(&format!(
            "build obj/{index}.o | obj/{index}.mod: dyndep | src/{index}.h\n"
        ));
    }
    source
}

fn dyndep_parse_benchmark(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("dyndep_parse");
    for edges in [100usize, 1_000, 10_000] {
        let source = generated_dyndep(edges);
        group.throughput(Throughput::Bytes(source.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(edges),
            &source,
            |bench, source| {
                bench.iter(|| parse_dyndep(source, Path::new("deps.dd")).unwrap());
            },
        );
    }
    group.finish();
}

fn generated_msvc_output(lines: usize) -> Vec<u8> {
    let mut output = String::with_capacity(lines * 48);
    for index in 0..lines {
        match index % 3 {
            0 => output.push_str(&format!(
                "Note: including file: include/header_{index}.h\r\n"
            )),
            1 => output.push_str(&format!("warning C{index}: useful compiler output\r\n")),
            _ => output.push_str(&format!("source_{index}.cc\r\n")),
        }
    }
    output.into_bytes()
}

fn msvc_filter_benchmark(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("msvc_filter");
    for lines in [100usize, 1_000, 10_000] {
        let output = generated_msvc_output(lines);
        group.throughput(Throughput::Bytes(output.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(lines),
            &output,
            |bench, output| {
                bench.iter(|| {
                    let mut includes = BTreeSet::new();
                    filter_msvc_output(output, "", &mut includes).unwrap()
                });
            },
        );
    }
    group.finish();
}

fn elide_middle_benchmark(criterion: &mut Criterion) {
    let inputs = [
        "01234567890123456789",
        "012345\x1b[0;35m67890123456789",
        "abcd\x1b[1;31mefg\x1b[0mhlkmnopqrstuvwxyz",
    ];
    criterion.bench_function("elide_middle/upstream_sweep", |bench| {
        bench.iter(|| {
            for input in inputs {
                for width in (1..=input.len()).rev() {
                    std::hint::black_box(elide_middle(input.as_bytes(), width));
                }
            }
        });
    });
}

criterion_group!(
    benches,
    parse_benchmark,
    dyndep_parse_benchmark,
    msvc_filter_benchmark,
    elide_middle_benchmark
);
criterion_main!(benches);
