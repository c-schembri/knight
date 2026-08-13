use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use knight_build::dyndep::parse_dyndep;
use knight_build::parse_manifest;
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

criterion_group!(benches, parse_benchmark, dyndep_parse_benchmark);
criterion_main!(benches);
