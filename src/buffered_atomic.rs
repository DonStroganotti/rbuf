use std::sync::atomic::{AtomicUsize, Ordering};

use crate::AtomicItem;

#[derive(Debug, Default)]
pub struct BufferedAtomic<T>
where
    T: Clone,
{
    pub(crate) ringbuffer: Vec<AtomicItem<T>>,
    /// The current index that will be written to
    /// reads will be done from write_index -1
    pub(crate) write_index: AtomicUsize,

    /// Mask used to wrap the write index using &
    pub(crate) mask: usize,
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
    fn test_seq_overflow() {
        let value: AtomicItem<usize> = AtomicItem::new();
        let _ = value.try_write(5); // initial value
        let _ = value.try_write(5); // initial value should drop here
        assert_eq!(value.drop_count.load(Ordering::SeqCst), 1);

        value.seq.store(usize::MAX - 1, Ordering::SeqCst);

        assert_eq!(value.seq.load(Ordering::SeqCst), usize::MAX - 1);

        // check if values keep being dropped correctly on wrap around
        let _ = value.try_write(10);
        assert_eq!(value.drop_count.load(Ordering::SeqCst), 2);
        let _ = value.try_write(15);
        assert_eq!(value.drop_count.load(Ordering::SeqCst), 3);
        assert_eq!(value.try_read(), Some(15));
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
        let buffer = Arc::new(BufferedAtomic::<String>::new(4));

        let write = buffer.clone();
        let read = buffer.clone();

        let wh = thread::spawn(move || {
            for i in 0..10000 {
                write.write(format!("Hello reader: {i}"));
            }
            write.write("Final value 🚂".to_string());
        });

        let rh = thread::spawn(move || {
            for _ in 0..1000 {
                let _ = read.read();
            }
        });

        while !wh.is_finished() || !rh.is_finished() {}

        assert_eq!(buffer.read(), format!("Final value 🚂"))
    }
}
