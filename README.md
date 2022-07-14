# Kanal
**Fastest sync and async channel that Rust deserves!**

## What is Kanal
Kanal is a Rust library to help programmers design effective programs in CSP model via providing featureful multi-producer multi-consumer channels.
This library focuses on bringing both sync and async API together to unify message passing between sync and async parts of Rust code in a performant manner.

**Performance is the main goal of Kanal.**

### Benchmark Results

Machine: AMD Ryzen Threadripper 2950X 16-Core Processor

Rust: `rustc 1.62.0`

Go: `go version go1.18.3 linux/amd64`

July 14 2022

[Benchmark codes](https://github.com/fereidani/rust-channel-benchmarks)

![Benchmark bounded channel with size 0](https://i.imgur.com/vEBirUw.png)
![Benchmark bounded channel with size 1](https://i.imgur.com/iDETIAK.png)
![Benchmark bounded channel with size n](https://i.imgur.com/qdjXzyh.png)
![Benchmark unbounded channel](https://i.imgur.com/idxEm3k.png)