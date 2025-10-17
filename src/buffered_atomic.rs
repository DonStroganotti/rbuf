use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicUsize, Ordering},
};

/// Stores the data for the ring buffer
#[repr(align(64))]
struct AtomicItem<T>
where
    T: Clone,
{
    value: UnsafeCell<std::mem::MaybeUninit<T>>,
    /// Sequence
    /// uneven = writing
    /// > 0 = initialized
    seq: AtomicUsize,
}

unsafe impl<T: Clone> Send for AtomicItem<T> {}
unsafe impl<T: Clone> Sync for AtomicItem<T> {}

impl<T> AtomicItem<T>
where
    T: Clone,
{
    pub fn new() -> Self {
        Self {
            value: UnsafeCell::new(MaybeUninit::uninit()),
            seq: AtomicUsize::new(0),
        }
    }

    /// Tries to write the value, returns Ok on success
    ///
    /// Returns the value: T on error
    pub fn try_write(&self, value: T) -> Result<(), T> {
        // Try to acquire this item for write access
        let seq = self.seq.load(Ordering::Acquire);

        // If sequence is uneven there's another writer already accessing this item
        if seq & 1 != 0 {
            return Err(value);
        }

        // try to get write access by attempting to increment to an uneven value
        let result = self
            .seq
            .compare_exchange(seq, seq + 1, Ordering::AcqRel, Ordering::Relaxed);

        // Failed to lock for writing
        if result.is_err() {
            return Err(value);
        }

        // SAFETY: we only modify the item if the compare_exchange is successful,
        // which guarantees that no other writer will be accessing it

        // If item has already been initialized we need to drop old value before writing new
        unsafe {
            // if sequence is > 0 the item has been initialized
            // TODO: if sequence wraps around back to 0 this will cause an issue
            if seq != 0 {
                (*self.value.get()).assume_init_drop();
            }

            // Write data to item
            (*self.value.get()).write(value);
        }

        // Complete write by setting seq to an even value
        self.seq.fetch_add(1, Ordering::Release);

        Ok(())
    }

    /// Tries to write the value, returns Ok on success
    pub fn try_read(&self) -> Option<T> {
        // Check if a writer is currently writing to this
        let seq = self.seq.load(Ordering::Acquire);

        // The value has to be initialized and not currently being written to,
        // otherwise we can't return anything
        if seq & 1 != 0 || seq == 0 {
            return None;
        }

        unsafe {
            let ret = (*self.value.get()).assume_init_ref().clone();

            let seq2 = self.seq.load(Ordering::Acquire);

            if seq != seq2 {
                return None;
            }

            // SAFETY: we only return a value if the seq hasn't changed from before and after read
            Some(ret)
        }
    }
}

pub struct BufferedAtomic<T>
where
    T: Clone,
{
    ringbuffer: Vec<AtomicItem<T>>,
    /// The current index that will be written to
    /// reads will be done from write_index -1
    write_index: AtomicUsize,

    /// Mask used to wrap the write index using &
    mask: usize,
}

impl<T> BufferedAtomic<T>
where
    T: Clone,
{
    /// Creates a new BufferedAtomic
    ///
    /// `size` has to be a power of 2
    pub fn new(size: usize) -> Self {
        assert!(size.is_power_of_two());

        let items: Vec<_> = (0..size).map(|_| AtomicItem::new()).collect();

        Self {
            ringbuffer: items,
            write_index: AtomicUsize::new(0),
            mask: size - 1,
        }
    }

    pub fn write(&self, value: T) {
        let mut value = value;
        loop {
            let idx = self.write_index.fetch_add(1, Ordering::Relaxed) & self.mask;
            let item = &self.ringbuffer[idx];

            let result = item.try_write(value);

            // if the write fails the value is returned in the error so we can use it again
            if let Err(ret_value) = result {
                value = ret_value;
                // Because there is no reason to write into the same index as another writer is doing,
                // our choices are to either increment index again until we find a free one, or drop this data immediately
                // by continuing the loop from the start we increment write_index again

                std::hint::spin_loop();
                continue;
            } else {
                break;
            }
        }
    }

    pub fn read(&self) -> T {
        loop {
            let idx = self.write_index.load(Ordering::Relaxed).wrapping_sub(1) & self.mask;
            let item = &self.ringbuffer[idx];

            let result = item.try_read();

            // return the value if there is one, otherwise spin loop
            if let Some(value) = result {
                return value;
            } else {
                std::hint::spin_loop();
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use std::{sync::Arc, thread};

    use super::*;

    #[test]
    fn test_item_write_read() {
        let value: AtomicItem<String> = AtomicItem::new();

        assert_eq!(value.try_read(), None);

        assert_eq!(value.try_write("Hello world".to_string()), Ok(()));

        assert_eq!(value.try_read(), Some("Hello world".to_string()));
    }

    #[test]
    fn test_item_multiple_read() {
        let value: AtomicItem<String> = AtomicItem::new();

        assert_eq!(value.try_write("Hello world".to_string()), Ok(()));

        assert_eq!(value.try_read(), Some("Hello world".to_string()));
        assert_eq!(value.try_read(), Some("Hello world".to_string()));
        assert_eq!(value.try_read(), Some("Hello world".to_string()));
        assert_eq!(value.try_read(), Some("Hello world".to_string()));
    }

    #[test]
    fn test_buffered_atomic_write_read() {
        let buffer: BufferedAtomic<String> = BufferedAtomic::new(4);

        buffer.write("Hello world".to_string());
        buffer.write("Hello world".to_string());
        buffer.write("Hello world".to_string());
        buffer.write("Hello world".to_string());
        buffer.write("Final hello world string!".to_string());

        assert_eq!(buffer.read(), "Final hello world string!".to_string());
    }

    #[test]
    fn test_read_write_thread() {
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

        wh.join().unwrap();
        rh.join().unwrap();
    }
}
