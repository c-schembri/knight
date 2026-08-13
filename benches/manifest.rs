use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use knight_build::parse_manifest;

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

criterion_group!(benches, parse_benchmark);
criterion_main!(benches);
