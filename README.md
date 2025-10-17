# rbuf — Buffered Atomics (Lock-Free Concurrent Buffer)

A high-performance, lock-free buffering library for Rust, designed for concurrent access with minimal overhead and flexible usage patterns.

## Overview

`BufferedAtomics` is a simple, fast, and versatile lock-free buffer implementation for concurrent producers and consumers.  
It provides atomic read/write access with minimal synchronization overhead, making it ideal for high-throughput systems such as telemetry, streaming, and logging.

---

## Features

- **Thread-Safe** – Built entirely on atomic operations; no locks or mutexes.
- **Buffered Design** – Efficient handling of concurrent reads/writes.
- **High Performance** – Optimized memory layout, cache-friendly, minimal contention.
- **Predictable Memory Usage** – Fixed-size preallocation.

---

## Example Usage

```rust
let buffer = Arc::new(BufferedAtomic::new(4));

let write = buffer.clone();
let read = buffer.clone();

let wh = thread::spawn(move || {
    for i in 0..10 {
        write.write(format!("Hello reader: {i}"));
    }
});

let rh = thread::spawn(move || {
    for _ in 0..10 {
        let _ = read.read();
    }
});
```
## Notes 
The older `RingBuffer` is deprecated. 

`BufferedAtomics` is a lot more flexible and performs better.
