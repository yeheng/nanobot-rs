//! Benchmark for vault placeholder scanner
//!
//! This benchmark compares the performance of cached vs non-cached regex compilation
//! to demonstrate the performance improvement from moving Regex::new() out of the
//! function body.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use nanobot_core::vault::{
    contains_placeholders, replace_placeholders, scan_placeholders, Placeholder,
};
use regex::Regex;
use std::collections::HashMap;

/// Original implementation with inline regex compilation (for comparison)
fn scan_placeholders_old(text: &str) -> Vec<Placeholder> {
    let re = Regex::new(r"\{\{vault:([a-zA-Z0-9_]+)\}\}").unwrap();
    re.captures_iter(text)
        .filter_map(|cap| {
            let full = cap.get(0)?;
            let key = cap.get(1)?;
            Some(Placeholder {
                key: key.as_str().to_string(),
                full_match: full.as_str().to_string(),
                start: full.start(),
                end: full.end(),
            })
        })
        .collect()
}

/// Original implementation with inline regex compilation (for comparison)
fn contains_placeholders_old(text: &str) -> bool {
    let re = Regex::new(r"\{\{vault:([a-zA-Z0-9_]+)\}\}").unwrap();
    re.is_match(text)
}

/// Original implementation with inline regex compilation (for comparison)
fn replace_placeholders_old(text: &str, replacements: &HashMap<String, String>) -> String {
    let re = Regex::new(r"\{\{vault:([a-zA-Z0-9_]+)\}\}").unwrap();
    re.replace_all(text, |cap: &regex::Captures| -> String {
        let key = &cap[1];
        replacements
            .get(key)
            .cloned()
            .unwrap_or_else(|| cap.get(0).unwrap().as_str().to_string())
    })
    .to_string()
}

fn benchmark_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("scan_placeholders");

    // Test with different text sizes
    let small_text = "Hello {{vault:key}} world";
    let medium_text =
        "Config: {{vault:db_host}}:{{vault:db_port}} with password {{vault:db_password}}";
    let large_text = "Use {{vault:api_key}} for authentication, connect to {{vault:db_host}}:{{vault:db_port}}, use {{vault:cache_host}} for caching, and {{vault:secret_token}} for signing. Also {{vault:smtp_user}} and {{vault:smtp_pass}} for email.";

    // Small text benchmark
    group.throughput(Throughput::Bytes(small_text.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("cached", "small"),
        small_text,
        |b, text| b.iter(|| scan_placeholders(black_box(text))),
    );
    group.bench_with_input(
        BenchmarkId::new("non-cached", "small"),
        small_text,
        |b, text| b.iter(|| scan_placeholders_old(black_box(text))),
    );

    // Medium text benchmark
    group.throughput(Throughput::Bytes(medium_text.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("cached", "medium"),
        medium_text,
        |b, text| b.iter(|| scan_placeholders(black_box(text))),
    );
    group.bench_with_input(
        BenchmarkId::new("non-cached", "medium"),
        medium_text,
        |b, text| b.iter(|| scan_placeholders_old(black_box(text))),
    );

    // Large text benchmark
    group.throughput(Throughput::Bytes(large_text.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("cached", "large"),
        large_text,
        |b, text| b.iter(|| scan_placeholders(black_box(text))),
    );
    group.bench_with_input(
        BenchmarkId::new("non-cached", "large"),
        large_text,
        |b, text| b.iter(|| scan_placeholders_old(black_box(text))),
    );

    group.finish();
}

fn benchmark_contains(c: &mut Criterion) {
    let mut group = c.benchmark_group("contains_placeholders");

    let text = "Hello {{vault:key}} world";

    group.throughput(Throughput::Bytes(text.len() as u64));
    group.bench_function("cached", |b| {
        b.iter(|| contains_placeholders(black_box(text)))
    });
    group.bench_function("non-cached", |b| {
        b.iter(|| contains_placeholders_old(black_box(text)))
    });

    group.finish();
}

fn benchmark_replace(c: &mut Criterion) {
    let mut group = c.benchmark_group("replace_placeholders");

    let text = "Config: {{vault:db_host}}:{{vault:db_port}} with password {{vault:db_password}}";
    let mut replacements = HashMap::new();
    replacements.insert("db_host".to_string(), "localhost".to_string());
    replacements.insert("db_port".to_string(), "5432".to_string());
    replacements.insert("db_password".to_string(), "secret".to_string());

    group.throughput(Throughput::Bytes(text.len() as u64));
    group.bench_function("cached", |b| {
        b.iter(|| replace_placeholders(black_box(text), black_box(&replacements)))
    });
    group.bench_function("non-cached", |b| {
        b.iter(|| replace_placeholders_old(black_box(text), black_box(&replacements)))
    });

    group.finish();
}

fn benchmark_iterations(c: &mut Criterion) {
    let mut group = c.benchmark_group("scan_iterations");

    let text = "Config: {{vault:db_host}}:{{vault:db_port}} with password {{vault:db_password}}";

    // Measure the improvement over multiple iterations
    group.throughput(Throughput::Elements(1000));
    group.bench_function("cached (1000 iterations)", |b| {
        b.iter(|| {
            for _ in 0..1000 {
                black_box(scan_placeholders(black_box(text)));
            }
        })
    });
    group.bench_function("non-cached (1000 iterations)", |b| {
        b.iter(|| {
            for _ in 0..1000 {
                black_box(scan_placeholders_old(black_box(text)));
            }
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    benchmark_scan,
    benchmark_contains,
    benchmark_replace,
    benchmark_iterations,
);
criterion_main!(benches);
