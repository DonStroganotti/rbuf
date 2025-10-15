use std::{
    cell::UnsafeCell,
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};

/// A slot in the ring buffer
///
/// Each slot contains:
/// - `write_state`: Atomic counter to track write operations (incremented at start and end of write)
/// - `buffer`: UnsafeCell containing raw bytes for this slot
struct Slot {
    /// The write state indicator helps us keep track of whether a buffer is being written to or not
    ///
    /// It is incremented by 1 when write is starting
    /// And Incremented by 1 again when write has finished
    ///
    /// When reading this state is checked before and after to make sure no thread has partially written data during read,
    write_state: AtomicUsize,

    /// Raw bytes stored by this ringbuffer
    pub buffer: UnsafeCell<Vec<u8>>,
}

impl Slot {
    /// Creates a new slot with the specified buffer size
    fn new(buffer_size: usize, clear_value: u8) -> Self {
        Self {
            buffer: UnsafeCell::new(vec![clear_value; buffer_size]),
            write_state: AtomicUsize::new(0),
        }
    }
}

// Safety: Slot is safe to send between threads because it only contains
// atomic types and UnsafeCell<Vec<u8>> which is properly synchronized
unsafe impl Send for Slot {}
unsafe impl Sync for Slot {}

/// A lock-free ring buffer implementation
///
/// Features:
/// - Thread-safe operations using atomic primitives
/// - No mutexes or locks, but operations may block if multiple threads contend for the same buffer index
///   - a sufficiently large ring buffer should almost never have this issue.
/// - Efficient memory usage with pre-allocated slots
/// - Clear data pattern for uninitialized slots
pub struct RingBuffer {
    /// Vector of slots that make up the ring buffer
    slots: Vec<Slot>,
    /// Atomic index tracking where the next write should occur
    write_idx: AtomicUsize,
    /// Pattern used to initialize new slots with clear data
    clear_value: u8,
}

impl RingBuffer {
    /// `width` is the size of the Slot buffers which contains the data
    ///
    /// `length` is how many `Slot`'s the ringbuffer stores
    pub fn new(buffer_size: usize, length: usize, clear_value: u8) -> Self {
        assert!(buffer_size != 0);
        assert!(length != 0);
        assert!(length.is_power_of_two());

        Self {
            slots: (0..length)
                .map(|_| Slot::new(buffer_size, clear_value))
                .collect(),
            write_idx: AtomicUsize::new(0),
            clear_value: clear_value,
        }
    }

    /// Write data into the next available slot, (truncates data if it doesn't fit)
    pub fn write_internal<T>(&self, data: T, clear: bool)
    where
        T: AsRef<[u8]>,
    {
        let data = data.as_ref();

        let idx = self.write_idx.fetch_add(1, Ordering::Relaxed) & (self.slots.len() - 1);
        let slot = &self.slots[idx];

        // Wait for slot to be free (even write_state)
        while !slot.write_state.load(Ordering::Acquire).is_multiple_of(2) {
            std::hint::spin_loop();
        }

        let buffer = slot.buffer.get();

        // Write to buffer
        unsafe {
            let buffer_len = (*buffer).len();
            // length of the slot buffer is used to prevent overflow when copying data
            let len = data.len().min(buffer_len);

            // 1. Increment to indicate that we are writing
            slot.write_state.fetch_add(1, Ordering::Release);

            // if clear is set then overwrite the buffer with clear_value
            if clear {
                ptr::write_bytes((*buffer).as_mut_ptr(), self.clear_value, buffer_len);
            }

            // 2. Copy data to buffer
            ptr::copy_nonoverlapping(data.as_ptr(), (*buffer).as_mut_ptr(), len);

            // 3. Increment to indicate that we done writing
            slot.write_state.fetch_add(1, Ordering::Release);
        }
    }

    pub fn write<T>(&self, data: T)
    where
        T: AsRef<[u8]>,
    {
        self.write_internal(data, false);
    }

    pub fn write_and_clear<T>(&self, data: T)
    where
        T: AsRef<[u8]>,
    {
        self.write_internal(data, true);
    }

    /// clones the internal buffer when it is not being written to
    pub fn read(&self) -> &[u8] {
        loop {
            let idx =
                self.write_idx.load(Ordering::Relaxed).saturating_sub(1) & (self.slots.len() - 1);
            let slot = &self.slots[idx];

            let write_state_before = slot.write_state.load(Ordering::Acquire);

            // uneven means the slot it is currently being written to
            if !write_state_before.is_multiple_of(2) {
                std::hint::spin_loop();
                continue;
            }

            let buffer = slot.buffer.get();

            // Read from buffer
            unsafe {
                let write_state_after = slot.write_state.load(Ordering::Acquire);

                // if the write_state hasn't changed we can safely return a fully written value
                if write_state_before == write_state_after {
                    return &*buffer;
                }
            }
        }
    }

    // Reads the data as a utf8 string
    pub fn read_str(&self) -> String {
        let data = self.read();
        String::from_utf8_lossy(&data)
            .trim_end_matches("\0")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, thread, time::Duration};

    use super::*;

    #[test]
    fn test() {
        let rbuf = Arc::new(RingBuffer::new(32, 16, 0));

        let rbuf2 = rbuf.clone();
        let rbuf3 = rbuf.clone();

        thread::spawn(move || {
            loop {
                rbuf.write_and_clear("potato");
                thread::sleep(Duration::from_millis(1));
            }
        });

        thread::spawn(move || {
            loop {
                rbuf2.write("apple bees");
                thread::sleep(Duration::from_millis(1));
            }
        });
        thread::sleep(Duration::from_millis(100));

        for _ in 0..100 {
            println!("{:?}", rbuf3.read_str());
            thread::sleep(Duration::from_millis(100));
        }
    }
}
