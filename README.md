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
The Kanal library is a Rust implementation of channels of the CSP (Communicating Sequential Processes) model, designed to assist programmers in creating efficient programs. This library provides multi-producer and multi-consumer channels with advanced features and lock-free one-shot channels that only allocate the size of a pointer in the heap, allowing for fast communication. The library has a focus on unifying message passing between synchronous and asynchronous portions of Rust code through a combination of synchronous and asynchronous APIs, while maintaining high performance.


## Why Kanal is faster?
1. Kanal employs a highly optimized composite technique for the transfer of objects. When the data size is less than or equal to the pointer size, it utilizes serialization, encoding the data as the pointer address. Conversely, when the data size exceeds the pointer size, the protocol employs a strategy similar to that utilized by the Golang programming language, utilizing direct memory access to copy objects from the sender's stack or write directly to the receiver's stack. This composite method not only eliminates unnecessary pointer access but also eliminates heap allocations for bounded(0) channels.
2. Kanal utilizes a specially tuned mutex for its channel locking mechanism, made possible by the predictable internal lock time of the channel. That said it's possible to use Rust standard mutex with `std-mutex` feature and Kanal will perform better than competitors with that feature too.
3. Utilizing Rust high-performance compiler and powerful LLVM backend with highly optimized memory access and deeply thought algorithms.

## Why use Kanal?

* Kanal is fast and efficient in communication
* Kanal can communicate in both sync and async and even between sync and async by providing methods to transform sender/receiver to other API.
* Kanal provides cleaner API in comparison with other rust libraries
* Similar to Golang you have access to `Close` function and you can broadcast the close signal from any instance of the channel, to close the channel for both sides.

## Why not use Kanal?

* Kanal developers are trying their best to audit the small codebase of Kanal and make sure there is no undefined behavior. Kanal is using Unsafe. If you are not ok with that in your project we suggest using safe-only alternatives.


### Benchmark Results
Results are based on how many messages can be passed in each scenario per second.

#### Test types:
1. Seq is sequentially writing and reading to a channel in the same thread.
2. SPSC is one receiver, and one sender and passing messages between them.
3. MPSC is multiple sender threads with only one receiver.
4. MPMC is multiple senders and multiple receivers communicating through the same channel.

#### Message types:
1. `usize` tests are transferring messages of size hardware pointer.
2. `big` tests are transferring messages of 8x the size of the hardware pointer.

N/A means that the test subject is unable to perform the test due to its limitations, Some of the test subjects don't have implementation for size 0 channels, MPMC or unbounded channels.

Machine: `AMD Ryzen Threadripper 2950X 16-Core Processor`<br />
Rust: `rustc 1.65.0 (897e37553 2022-11-02)`<br />
Go: `go version go1.19.3 linux/amd64`<br />
OS (`uname -a`): `Linux 5.15.0-52-generic #58~20.04.1-Ubuntu SMP Thu Oct 13 13:09:46 UTC 2022 x86_64`<br />
Date: Nov 13, 2022

[Benchmark codes](https://github.com/fereidani/rust-channel-benchmarks)

![Benchmarks](https://i.imgur.com/i10Ayjw.png)

#### Why async outperforms sync in some tests?
In certain tests, asynchronous communication may exhibit superior performance compared to synchronous communication. This can be attributed to the context-switching performance of libraries such as tokio, which, similar to Golang, utilize context-switching within the same thread to switch to the next coroutine when a message is ready on a channel. This approach is more efficient than communicating between separate threads. This same principle applies to asynchronous network applications, which generally exhibit better performance compared to synchronous implementations. As the channel size increases, one may observe improved performance in synchronous benchmarks, as the sending threads are able to push data directly to the channel queue without requiring awaiting blocking/suspending signals from receiving threads.