use std::{
    cell::UnsafeCell,
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

/// A slot in the ring buffer
///
/// Each slot contains:
/// - `write_state`: Atomic counter to track write operations (incremented at start and end of write)
/// - `buffer`: UnsafeCell containing raw bytes for this slot
#[repr(align(64))] // 64 byte aligned improves performance, tested on Windows 11 - AMD 3950x
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
    _padding: [u8; 32], // Adjust based on actual struct size
}

impl Slot {
    /// Creates a new slot with the specified buffer size
    fn new(buffer_size: usize, clear_value: u8) -> Self {
        Self {
            buffer: UnsafeCell::new(vec![clear_value; buffer_size]),
            write_state: AtomicUsize::new(0),
            _padding: Default::default(),
        }
    }
}

// Safety: Slot is safe to send between threads because it only contains
// atomic types and UnsafeCell<Vec<u8>> which is properly synchronized
unsafe impl Send for Slot {}
unsafe impl Sync for Slot {}

/// A fixed size, lock-free ring buffer implementation
///
/// ## Features:
/// - Thread-safe operations using atomic primitives
/// - No mutexes or locks, but operations may block if multiple threads contend for the same buffer index
///   - a sufficiently large ring buffer should almost never have this issue.
/// - Efficient memory usage with pre-allocated slots
///
/// ## Note:
/// - Writing too quickly will result in the index looping back and overwriting old, possibly unread data.
/// - This buffer is designed for high-throughput scenarios where occasional data loss is acceptable,
///   rather than systems requiring guaranteed delivery of every single write.
/// - Readers should expect to see the most recently written data, but may miss intermediate writes
///   if the writer outpaces the reader.
///
pub struct RingBuffer {
    /// Vector of slots that make up the ring buffer
    slots: Vec<Slot>,
    /// Atomic index tracking where the next write should occur
    write_idx: AtomicUsize,
    /// Pattern used to initialize new slots with clear data
    clear_value: u8,
    /// Because the length/slot count has to be a power of 2
    /// then we can use length -1 as a mask to wrap values without modulo %
    mask: usize,
    /// Slot buffer size
    buffer_size: usize,
}

impl RingBuffer {
    /// Creates a new RingBuffer with the specified buffer size and length.
    ///
    /// # Arguments
    /// * `buffer_size` - The size of each Slot's internal buffer (in bytes)
    /// * `length` - The number of Slots in the ring buffer (must be a power of two)
    /// * `clear_value` - The value to initialize each slot's buffer with
    ///
    /// # Panics
    /// * If `buffer_size` is zero
    /// * If `length` is zero
    /// * If `length` is not a power of two
    pub fn new(buffer_size: usize, length: usize, clear_value: u8) -> Self {
        assert!(buffer_size != 0);
        assert!(length != 0);
        assert!(length.is_power_of_two());

        Self {
            slots: (0..length)
                .map(|_| Slot::new(buffer_size, clear_value))
                .collect(),
            write_idx: AtomicUsize::new(0),
            clear_value,
            mask: length - 1,
            buffer_size,
        }
    }

    /// Write data into the next available slot, truncating data if it doesn't fit
    /// into the `buffer_size` of the `Slot`
    ///
    /// This method atomically acquires the next available slot in the ring buffer,
    /// writes the provided data to that slot, and handles synchronization between
    /// multiple writer threads using atomic operations.
    ///
    /// # Parameters
    /// * `data` - The data to write, implementing `AsRef<[u8]>`
    /// * `clear` - If true, clears the buffer slot with the configured clear value
    ///   before writing new data
    ///
    /// # Behavior
    /// 1. Acquires the next available slot using atomic fetch-add operation
    /// 2. Waits for the slot to be free (write_state is even number)
    /// 3. Writes data to the buffer, truncating if necessary to fit the slot size
    /// 4. Uses atomic operations to ensure thread-safe writing:
    ///    - First increment: indicates writing has started
    ///    - Second increment: indicates writing is complete
    ///
    /// # Safety
    /// This function uses unsafe code for memory operations and assumes:
    /// - The buffer pointer is valid
    /// - Data length doesn't exceed buffer capacity
    /// - Atomic ordering constraints are maintained
    #[inline]
    fn write_internal<T>(&self, data: T, clear: bool)
    where
        T: AsRef<[u8]>,
    {
        let data = data.as_ref();

        loop {
            let idx = self.write_idx.fetch_add(1, Ordering::AcqRel) & self.mask;
            let slot = &self.slots[idx];

            // Wait for slot to be free (even write_state)
            if !slot.write_state.load(Ordering::Acquire).is_multiple_of(2) {
                // Goes back to start of loop to check if the next index is available
                continue;
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

            break;
        }
    }

    /// Write data into the next available slot without clearing the buffer first
    ///
    /// This function writes the provided data to the next available slot in the
    /// ring buffer. The existing contents of the buffer slot are preserved.
    ///
    /// # Parameters
    /// * `data` - The data to write, implementing `AsRef<[u8]>`
    ///
    /// # Example
    /// ```
    /// use rbuf::RingBuffer;
    /// let ringbuffer = RingBuffer::new(1024, 32, 0);
    /// ringbuffer.write("Hello");
    /// ```
    /// ## Note:
    /// - Writing too quickly will result in the index looping back and overwriting old, possibly unread data.
    /// - This buffer is designed for high-throughput scenarios where occasional data loss is acceptable,
    #[inline]
    pub fn write<T>(&self, data: T)
    where
        T: AsRef<[u8]>,
    {
        self.write_internal(data, false);
    }

    /// Write data into the next available slot, clearing the buffer first
    ///
    /// This function writes the provided data to the next available slot in the
    /// ring buffer. Before writing, it clears the entire buffer slot with the
    /// configured clear value (typically zero bytes).
    ///
    /// # Parameters
    /// * `data` - The data to write, implementing `AsRef<[u8]>`
    ///
    /// # Example
    /// ```
    /// use rbuf::RingBuffer;
    /// let ringbuffer = RingBuffer::new(1024, 32, 0);
    /// ringbuffer.write_and_clear("Hello");
    /// ```
    /// ## Note:
    /// - Writing too quickly will result in the index looping back and overwriting old, possibly unread data.
    /// - This buffer is designed for high-throughput scenarios where occasional data loss is acceptable,
    #[inline]
    pub fn write_and_clear<T>(&self, data: T)
    where
        T: AsRef<[u8]>,
    {
        self.write_internal(data, true);
    }

    /// Read data from the most recently written slot
    ///
    /// This function atomically reads the data from the most recently written slot
    /// in the ring buffer. It ensures thread safety by checking the write state
    /// to verify that the slot is not currently being written to.
    ///
    /// # Behavior
    /// 1. Calculates the index of the most recently written slot
    /// 2. Checks if the slot is currently being written to (odd write_state)
    /// 3. If writing is in progress, spins until the slot becomes available
    /// 4. Writes data to the output_buffer
    /// 5. Verifies that the slot content hasn't changed during read operation, if not it will try to read the data again
    ///
    /// # Safety
    /// This function uses unsafe code for memory operations and assumes:
    /// - The buffer pointer is valid
    /// - Atomic ordering constraints are maintained
    /// ## Note:
    /// - This buffer is designed for high-throughput scenarios where occasional data loss is acceptable,
    ///   rather than systems requiring guaranteed delivery of every single write.
    /// - Readers should expect to see the most recently written data, but may miss intermediate writes
    ///   if the writer outpaces the reader.
    #[inline]
    pub fn read_to_buf(&self, output_buffer: &mut Vec<u8>) {
        // Loop until we are able to fully read the buffer
        loop {
            // write index - 1 is the last written to Slot
            let idx = self.write_idx.load(Ordering::Acquire).wrapping_sub(1) & self.mask;
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
                // store the return data before checking write state as it is possible a write happens during the clone.
                output_buffer
                    .as_mut_ptr()
                    .copy_from_nonoverlapping((*buffer).as_mut_ptr(), self.buffer_size);

                let write_state_after = slot.write_state.load(Ordering::Acquire);

                // SAFETY: if the write_state hasn't changed we successfully copied all of the data
                if write_state_before == write_state_after {
                    break;
                }
            }
        }
    }

    /// Read data from the most recently written slot
    ///
    /// This function atomically reads the data from the most recently written slot
    /// in the ring buffer. It ensures thread safety by checking the write state
    /// to verify that the slot is not currently being written to.
    ///
    /// # Returns
    /// A copy of the data in the buffer
    ///
    /// # Behavior
    /// 1. Calculates the index of the most recently written slot
    /// 2. Checks if the slot is currently being written to (odd write_state)
    /// 3. If writing is in progress, spins until the slot becomes available
    /// 4. Verifies that the slot content hasn't changed during read operation
    /// 5. Returns a copy of the buffer data
    ///
    /// # Safety
    /// This function uses unsafe code for memory operations and assumes:
    /// - The buffer pointer is valid
    /// - Atomic ordering constraints are maintained
    /// ## Note:
    /// - This buffer is designed for high-throughput scenarios where occasional data loss is acceptable,
    ///   rather than systems requiring guaranteed delivery of every single write.
    /// - Readers should expect to see the most recently written data, but may miss intermediate writes
    ///   if the writer outpaces the reader.
    #[inline]
    pub fn read(&self) -> Vec<u8> {
        let mut output_buffer: Vec<u8> = vec![self.clear_value; self.buffer_size];
        self.read_to_buf(&mut output_buffer);
        output_buffer
    }

    /// Read data from the most recently written slot as a UTF-8 string
    ///
    /// This function reads the most recently written data from the ring buffer
    /// and converts it to a UTF-8 string, trimming trailing null bytes.
    ///
    /// # Returns
    /// A String containing the decoded UTF-8 data
    ///
    /// # Behavior
    /// 1. Calls the `read()` method to get the raw byte data
    /// 2. Converts the byte slice to a UTF-8 string using lossy conversion
    /// 3. Trims trailing null bytes from the string
    /// 4. Returns the resulting String
    ///
    /// # Example
    /// ```
    /// use rbuf::RingBuffer;
    /// let ringbuffer = RingBuffer::new(1024, 32, 0);
    /// ringbuffer.write("Hello World");
    /// let text = ringbuffer.read_str(); // Returns "Hello World"
    /// ```
    /// ## Note:
    /// - This buffer is designed for high-throughput scenarios where occasional data loss is acceptable,
    ///   rather than systems requiring guaranteed delivery of every single write.
    /// - Readers should expect to see the most recently written data, but may miss intermediate writes
    ///   if the writer outpaces the reader.
    #[inline]
    pub fn read_str(&self) -> String {
        let data = self.read();
        String::from_utf8_lossy(&data)
            .trim_end_matches("\0")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, thread};

    use super::*;

    #[test]
    fn test_basic_write_read() {
        let ringbuffer = RingBuffer::new(128, 32, 0);

        let data = b"Hello World";
        ringbuffer.write(data);

        let read_data = ringbuffer.read();
        assert_eq!(&read_data[..data.len()], data);
    }

    #[test]
    fn test_overwrite() {
        let ringbuffer = RingBuffer::new(128, 1, 0);

        let data = b"Hello World";

        // Write initial data
        ringbuffer.write(data);

        // Write with clear - should overwrite with zeros
        ringbuffer.write(b"World");

        let read_data = ringbuffer.read();
        assert_eq!(&read_data[..data.len()], b"World World");
    }

    #[test]
    fn test_read_str() {
        let ringbuffer = RingBuffer::new(128, 32, 0);

        ringbuffer.write(b"Hello World\0\0");
        let result = ringbuffer.read_str();
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_multiple_writes() {
        let ringbuffer = RingBuffer::new(1024, 32, 0);

        let data1 = b"First";
        let data2 = b"Second";
        let data3 = b"Third";

        ringbuffer.write(data1);
        ringbuffer.write(data2);
        ringbuffer.write(data3);

        // Should read the last written data
        let read_data = ringbuffer.read();
        assert_eq!(&read_data[..data3.len()], data3);
    }

    #[test]
    fn test_thread_safety() {
        let ringbuffer = Arc::new(RingBuffer::new(1024, 32, 0));
        let mut handles = vec![];

        // Spawn multiple threads writing to the buffer
        for i in 0..10 {
            let rb = Arc::clone(&ringbuffer);
            let handle = thread::spawn(move || {
                let data = format!("Thread {} data", i).into_bytes();
                rb.write(data);
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().unwrap();
        }

        // Read data - should be from one of the threads
        let read_data = ringbuffer.read();
        assert!(!read_data.is_empty());
    }

    #[test]
    fn test_buffer_truncation() {
        let ringbuffer = RingBuffer::new(8, 1, 0); // Small buffer size

        // Write more data than fits in the slot
        let large_data = b"Hello World This is too long";
        ringbuffer.write(large_data);

        let read_data = ringbuffer.read();
        // Should only contain first 8 bytes
        assert_eq!(read_data, b"Hello Wo");
    }

    #[test]
    fn test_empty_buffer() {
        let ringbuffer = RingBuffer::new(128, 32, 0);

        // Read from empty buffer should return empty slice
        let read_data = ringbuffer.read();
        assert_eq!(read_data.len(), 128);
    }

    #[test]
    fn test_store_struct_pointer() {
        let ringbuffer = RingBuffer::new(32, 32, 0);

        struct Test {
            a: u32,
            b: u32,
            c: String,
        }

        let data = Box::into_raw(Box::new(Test {
            a: 1,
            b: 5,
            c: "test".to_string(),
        }));

        let ptr_value = data as usize;

        let pointer_bytes = ptr_value.to_ne_bytes();

        ringbuffer.write(pointer_bytes);

        let read_bytes = &ringbuffer.read()[..pointer_bytes.len()];

        let ptr_value = usize::from_ne_bytes(read_bytes.try_into().unwrap()) as *mut Test;

        unsafe {
            assert_eq!((*ptr_value).a, 1);
            assert_eq!((*ptr_value).b, 5);
            assert_eq!((*ptr_value).c, "test".to_string());

            // drop pointer
            let _ = Box::from_raw(ptr_value);
        }
    }
}
