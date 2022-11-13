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
Kanal is a Rust library to help programmers design effective programs in the CSP model by providing featureful multi-producer multi-consumer channels.
This library focuses on bringing both sync and async API together to unify message passing between sync and async parts of Rust code in a performant manner.

> [Kanal is in pre-release and should not be used in production yet.](https://crates.io/crates/kanal)

**Performance is the main goal of Kanal.**

## Why Kanal is faster?
1. Kanal is using direct memory access to copy objects from the stack of the sender or write to the stack of the receiver the same as Golang, this eliminates any heap allocation need for bounded(0) channels and also reduces stack allocation substantially too.
2. Kanal is using specially tuned mutex for its channel lock, it is possible because the channel's internal lock time is predictable. That said the mutex is implemented with eventual fairness.
3. Rust amazing compiler

## Why use Kanal?

* Kanal is super fast and efficient in communication
* Kanal can communicate in both sync and async and even between sync and async
* Kanal provides cleaner API in comparison with other rust libraries
* Like Golang you have access to `Close` function and you can broadcast that signal from any instance of the channel.

## Why not use Kanal?

* We are trying our best to audit our small codebase and make sure there is no undefined behavior. Kanal is using Unsafe. If you are not ok with that in your project we suggest using safe-only alternatives.


### Benchmark Results
Results are based on how many messages can be passed in each scenario per second.

#### Test types:
1. seq is sequentially writing and reading to a channel in the same thread.
2. spsc is launching 2 threads, one receiver, and one sender and passing messages between them.
3. mpsc is launching multiple sender threads with only one receiver.
4. mpmc is launching the same count of multiple senders and receivers communicating through the same channel altogether.

#### Message types:
1. empty tests are those tests that are passing zero-sized messages like notifications to receivers.
2. usize tests are those tests that are passing messages of register size to receivers.
3. big tests are those tests that are passing messages of 4x the size of the register to receivers, for example, 32 bytes(4x8) structure for x64 systems.

N/A means that the test subject can't perform the test because of its limitations, for example, some libraries don't have support for size 0 channels or MPMC, and Golang does not support unbounded channels too.

Machine: `AMD Ryzen Threadripper 2950X 16-Core Processor`<br />
Rust: `rustc 1.65.0 (897e37553 2022-11-02)`<br />
Go: `go version go1.19.3 linux/amd64`<br />
OS (`uname -a`): `Linux 5.15.0-52-generic #58~20.04.1-Ubuntu SMP Thu Oct 13 13:09:46 UTC 2022 x86_64`<br />
Date: Nov 13, 2022

[Benchmark codes](https://github.com/fereidani/rust-channel-benchmarks)

![Benchmarks](https://i.imgur.com/i10Ayjw.png)

#### Why in some tests async is much faster than sync?
It's because of Tokio's context-switching performance, like Golang, Tokio context-switch in the same thread to the next coroutine when the channel message is ready which is much cheaper than communicating between different threads, It's the same reason why async network applications usually perform better than sync implementations.
As channel size grows you see better performance in sync benchmarks because channel sender threads can push their data directly to the channel queue and don't need to wait for signals from receivers threads.