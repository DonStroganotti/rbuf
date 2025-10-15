# rbuf - Lock-Free Ring Buffer

A high-performance, lock-free ring buffer implementation in Rust designed for concurrent access patterns.

## Features

- **Thread-Safe**: Uses atomic operations for synchronization without mutexes
- **Lock-Free**: No blocking operations, uses spin-waiting for contention resolution
- **Memory Efficient**: Pre-allocated slots with fixed memory usage
- **Zero-Cost Abstractions**: Optimized for performance-critical applications
- **Flexible**: Supports both clear and non-clear write modes

## Usage

```rust
use rbuf::RingBuffer;

// Create a ring buffer with 32 slots, each 1024 bytes
let ringbuffer = RingBuffer::new(1024, 32, 0);

// Write data
ringbuffer.write("Hello World");

// Read data
let data = ringbuffer.read();
let text = ringbuffer.read_str(); // UTF-8 string with null-byte trimming
```

## API

### `RingBuffer::new(buffer_size: usize, length: usize, clear_value: u8)`

Creates a new ring buffer with specified parameters:
- `buffer_size`: Size of each slot's internal buffer (in bytes)
- `length`: Number of slots in the ring buffer (must be power of two)
- `clear_value`: Value to initialize each slot with

### `write<T>(data: T)`

Writes data to the next available slot without clearing the buffer first.

### `write_and_clear<T>(data: T)`

Writes data to the next available slot, clearing the buffer first.

### `read() -> &[u8]`

Reads data from the most recently written slot, ensuring atomicity.

### `read_str() -> String`

Reads data as a UTF-8 string with trailing null bytes trimmed.

## Performance

- Uses 64-byte alignment for improved cache performance
- Atomic operations for synchronization
- Fixed size
- Minimal memory allocations
- Designed for high-throughput scenarios

## Safety

The implementation uses unsafe code for memory operations but ensures:
- Thread-safe access through atomic primitives
- Proper memory layout with alignment
- Bounds checking during buffer operations
- Clear data patterns for uninitialized slots

## Requirements

- Target platform supporting atomic operations

## License

MIT License - see LICENSE file for details.