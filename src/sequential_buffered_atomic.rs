use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::{BufferedAtomic, atomic_item::AtomicItem};

/// # BufferedAtomicSeq
///
/// This is a sequential version of the `BufferedAtomic`.
///
/// It guarantees that all writes and reads will happen in order, and no data will be lost.
///
/// This also means that if writers outpace readers they will have to wait before they can write new data
pub struct BufferedAtomicSeq<T: Clone> {
    ringbuffer: Vec<AtomicItem<T>>,

    write_cursor: AtomicUsize,
    read_cursor: AtomicUsize,

    mask: usize, // for fast modulo
}

impl<T> BufferedAtomicSeq<T>
where
    T: Clone,
{
    pub fn new(size: usize) -> Self {
        assert!(size.is_power_of_two(), "size must be power of two");

        let ringbuffer = (0..size).map(|_| AtomicItem::<T>::new()).collect();
        Self {
            ringbuffer,
            write_cursor: AtomicUsize::new(0),
            read_cursor: AtomicUsize::new(0),
            mask: size - 1,
        }
    }

    /// Write returns an Err if the write "cursor" is trying to overwrite the read cursor
    pub fn write(&self, value: T) -> Result<(), ()> {
        let size = self.ringbuffer.len();
        let write_pos = self.write_cursor.load(Ordering::Acquire);
        let read_pos = self.read_cursor.load(Ordering::Acquire);

        // full if next would overwrite unread
        if write_pos.wrapping_sub(read_pos) == size {
            return Err(()); // full
        }

        let slot = &self.ringbuffer[write_pos & self.mask];

        if slot.try_write(value).is_err() {
            return Err(());
        }

        // publish write (Release ensures visibility)
        self.write_cursor
            .store(write_pos.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Returns none if write and read cursor are the same value == no data in buffer
    pub fn read(&self) -> Option<T> {
        let read_pos = self.read_cursor.load(Ordering::Relaxed);
        let write_pos = self.write_cursor.load(Ordering::Acquire);

        if read_pos == write_pos {
            return None; // empty
        }

        let slot = &self.ringbuffer[read_pos & self.mask];

        let value = slot.try_read()?;
        self.read_cursor
            .store(read_pos.wrapping_add(1), Ordering::Release);
        Some(value)
    }
}
#[cfg(test)]
mod tests {

    use std::{sync::Arc, thread, time::Duration};

    use super::*;

    #[test]
    fn test_read_write() {
        let buffer = BufferedAtomicSeq::<usize>::new(4);

        let _ = buffer.write(15);

        assert_eq!(buffer.read(), Some(15));
        assert_eq!(buffer.read(), None); // the single available item has been read, so now it returns none
    }

    #[test]
    fn test_write_full() {
        let buffer = BufferedAtomicSeq::<usize>::new(1);
        assert!(buffer.write(1).is_ok());
        assert!(buffer.write(2).is_err());
    }

    #[test]
    fn test_read_write_multiple() {
        let buffer = BufferedAtomicSeq::<usize>::new(16);

        for i in 0..16 {
            let res = buffer.write(i);
            println!("{i} - {:?}", res);
        }

        for i in 0..16 {
            println!("{:?}", buffer.read());
        }
    }

    #[test]
    fn test_mt_order() {
        let buffer = Arc::new(BufferedAtomicSeq::<usize>::new(16));
        let write = buffer.clone();
        let read = buffer.clone();

        let max = 1000;

        let wh = thread::spawn(move || {
            let mut counter = 0;
            while counter < max {
                if write.write(counter).is_ok() {
                    counter += 1;
                } else {
                    std::hint::spin_loop();
                }
            }
            counter
        });

        let rh = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            let mut counter = 0;
            while counter < max {
                if let Some(value) = read.read()
                    && value == counter
                {
                    counter += 1;
                } else {
                    std::hint::spin_loop();
                }
            }
            counter
        });

        while !wh.is_finished() || !rh.is_finished() {
            thread::yield_now();
        }

        assert_eq!(max, wh.join().expect("Expected a value"));
        assert_eq!(max, rh.join().expect("Expected a value"));
    }
}
