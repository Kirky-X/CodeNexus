//! 可复现证据：证明 Rust `regex` crate 对 strix 报告 vuln-0001 的 PoC 免疫。
//!
//! 用途：配合 `docs/security/strix-codenexus_679c-triage.md`。独立可运行：
//!
//! ```text
//! cargo new redos_proof && cp strix-codenexus_679c-redos-proof.rs redos_proof/src/main.rs
//! # Cargo.toml: regex = "1"
//! cargo run --release
//! ```
//!
//! 三个证明：
//! 1. 报告主推 PoC（lookbehind / 反向引用）在编译期即被 regex crate 拒绝
//! 2. 嵌套量词 `(a+)+b`（最坏情况不匹配）在 regex crate 下是线性时间
//! 3. 嵌套量词 `(a+)+$` 匹配场景下提前短路

use regex::Regex;
use std::time::Instant;

fn main() {
    println!("=== 证明 1: regex crate 拒绝漏洞报告 PoC 里的 lookbehind 模式 ===\n");
    let poc_patterns = [
        r"(?<=(a+))+a+", // lookbehind — 报告声称这会触发 ReDoS
        r"(?<=a)b",      // 简单 lookbehind
        r"(a+)\1",       // backreference
    ];
    for p in &poc_patterns {
        match Regex::new(p) {
            Ok(_) => println!("  [编译成功] {p:?}"),
            Err(e) => println!(
                "  [编译被拒] {p:?}\n      -> {}",
                e.to_string().lines().next().unwrap_or("")
            ),
        }
    }

    println!("\n=== 证明 2: 嵌套量词 (a+)+b 在 regex crate 下是【线性时间】, 不是指数 ===\n");
    println!("  (报告声称此模式 'fall back to backtracking NFA' 造成灾难性回溯)\n");
    // 回溯引擎下, (a+)+b 匹配 "aaa...a" (不匹配, 缺少 b) 会指数爆炸；Pike VM 下严格线性。
    let re = Regex::new(r"(a+)+b").unwrap();
    for n in [1000usize, 2000, 4000, 8000, 16000, 32000] {
        let hay = "a".repeat(n); // 无尾部 b -> 回溯引擎的最坏情况
        let t = Instant::now();
        let m = re.is_match(&hay);
        let elapsed_us = t.elapsed().as_micros();
        println!(
            "  len={:>6}  is_match={:<5}  耗时 {:>8} µs",
            n, m, elapsed_us
        );
    }
    println!(
        "\n  线性判定: 长度翻倍耗时也近似翻倍; 若是指数, 32000 会比 1000 慢 2^22 倍(不可能完成)"
    );

    println!("\n=== 证明 3: ReportPoC 的另一种嵌套 (a+)+$ ===\n");
    let re2 = Regex::new(r"(a+)+$").unwrap();
    for n in [1000usize, 5000, 20000] {
        let hay = "a".repeat(n);
        let t = Instant::now();
        let _ = re2.is_match(&hay);
        println!("  len={:>6}  耗时 {:>8} µs", n, t.elapsed().as_micros());
    }
}
