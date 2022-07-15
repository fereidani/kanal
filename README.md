# Kanal
**Fastest sync and async channel that Rust deserves!**

## What is Kanal
Kanal is a Rust library to help programmers design effective programs in CSP model via providing featureful multi-producer multi-consumer channels.
This library focuses on bringing both sync and async API together to unify message passing between sync and async parts of Rust code in a performant manner.

**Performance is the main goal of Kanal.**

### Benchmark Results

Machine: `AMD Ryzen Threadripper 2950X 16-Core Processor`<br />
Rust: `rustc 1.62.0`<br />
Go: `go version go1.18.3 linux/amd64`<br />
OS (`uname -a`): `Linux 5.13.0-35-generic #40~20.04.1-Ubuntu SMP Mon Mar 7 09:18:32 UTC 2022 x86_64 x86_64 x86_64 GNU/Linux`<br />
Date: July 15, 2022

[Benchmark codes](https://github.com/fereidani/rust-channel-benchmarks)

![Benchmark bounded channel with size 0](https://i.imgur.com/NOP91jD.png)
![Benchmark bounded channel with size 1](https://i.imgur.com/MpsuWIi.png)
![Benchmark bounded channel with size n](https://i.imgur.com/9ebey2h.png)
![Benchmark unbounded channel](https://i.imgur.com/WgrFRtK.png)
