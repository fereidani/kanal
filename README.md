# Kanal
**The fast sync and async channel that Rust deserves!**

[![Crates.io][crates-badge]][crates-url]
[![Documentation][doc-badge]][doc-url]
[![MIT licensed][mit-badge]][mit-url]

[crates-badge]: https://img.shields.io/crates/v/kanal.svg
[crates-url]: https://crates.io/crates/kanal
[mit-badge]: https://img.shields.io/badge/license-MIT-blue.svg
[mit-url]: https://github.com/fereidani/kanal/blob/master/LICENSE
[doc-badge]: https://docs.rs/kanal/badge.svg
[doc-url]: https://docs.rs/kanal

## What is Kanal
Kanal is a Rust library to help programmers design effective programs in CSP model via providing featureful multi-producer multi-consumer channels.
This library focuses on bringing both sync and async API together to unify message passing between sync and async parts of Rust code in a performant manner.

> [Kanal is in pre-release and should not be used in production yet.](https://crates.io/crates/kanal)

**Performance is the main goal of Kanal.**

## Why Kanal is faster?
1. Kanal is using direct memory access to copy objects from stack of the sender or write to stack of the receiver exactly the same as Golang, this eliminates any heap allocation need for bounded(0) channels and also reduce stack allocation substantially too.
2. Kanal is using specially tuned mutex for its channel lock, it is possible because channel internal lock time is predictable. That said the mutex is implemeneted with eventual fairness.
3. Rust amazing compiler

## Why to use Kanal?

* Kanal is super fast and efficient in communication
* Kanal can communicate in both sync and async and even between sync and async
* Kanal provide cleaner API in comparison with other rust libraries
* Like Golang you have access to Close function and you can broadcast that signal from any instance of the channel.

## Why not to use Kanal?

* We are trying our best to audit our small codebase and make sure there is no undefined behavior. Kanal is using Unsafe. If you are not ok with that in your project we suggest using safe-only alternatives.


### Benchmark Results
Results are normalized based on Kanal sync results, so 60x means the test for that framework takes 60 times slower than Kanal.

Machine: `AMD Ryzen Threadripper 2950X 16-Core Processor`<br />
Rust: `rustc rustc 1.64.0`<br />
Go: `go version go1.19.2 linux/amd64`<br />
OS (`uname -a`): `Linux 5.13.0-35-generic #40~20.04.1-Ubuntu SMP Mon Mar 7 09:18:32 UTC 2022 x86_64`<br />
Date: Oct 16, 2022

[Benchmark codes](https://github.com/fereidani/rust-channel-benchmarks)

![Benchmarks](https://i.imgur.com/gHfk5fy.png)
