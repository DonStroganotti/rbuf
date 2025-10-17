use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
};

#[repr(u8)]
enum ItemStatus {
    Accessible = 0,
    Writing = 1 << 0,
    Reading = 1 << 1,
}

impl From<ItemStatus> for u8 {
    fn from(value: ItemStatus) -> Self {
        value as u8
    }
}

/// Stores the data for the ring buffer
#[repr(align(64))]
struct AtomicItem<T>
where
    T: Clone,
{
    value: UnsafeCell<std::mem::MaybeUninit<T>>,
    /// This used as a bitfield for ItemStatus
    status: AtomicU8,
    /// set to true when value has been initialized
    initialized: UnsafeCell<bool>,
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
            status: AtomicU8::new(ItemStatus::Accessible.into()),
            initialized: UnsafeCell::new(false),
        }
    }

    /// Tries to write the value, returns Ok on success
    ///
    /// Returns the value: T on error
    pub fn try_write(&self, value: T) -> Result<(), T> {
        // Try to acqurie this item for write access
        let result = self.status.compare_exchange(
            ItemStatus::Accessible.into(),
            ItemStatus::Writing.into(),
            Ordering::Acquire,
            Ordering::Relaxed,
        );

        // Failed to lock for writing
        if result.is_err() {
            return Err(value);
        }

        // SAFETY: we only modify the item if the compare_exchange is successful,
        // which guarantees that no other reader or writer will be accessing it

        // If item has already been initialized we need to drop old value before writing new
        if unsafe { *self.initialized.get() } {
            unsafe {
                (*self.value.get()).assume_init_drop();
            };
        }

        // Write data to item
        unsafe {
            (*self.value.get()).write(value);
        }
        // mark as initialized

        unsafe { *self.initialized.get() = true }

        // restore status to be accessible
        self.status
            .store(ItemStatus::Accessible.into(), Ordering::Relaxed);

        Ok(())
    }

    /// Tries to write the value, returns Ok on success
    pub fn try_read(&self) -> Option<T> {
        // Try to acquire this item for read access
        let result = self.status.compare_exchange(
            ItemStatus::Accessible.into(),
            ItemStatus::Reading.into(),
            Ordering::Acquire,
            Ordering::Relaxed,
        );

        // Failed to lock for writing
        if result.is_err() {
            return None;
        }

        // The value has to be initialized, otherwise we can't return anything
        if unsafe { !*self.initialized.get() } {
            // restore status to be accessible
            self.status
                .store(ItemStatus::Accessible.into(), Ordering::Relaxed);
            return None;
        }

        // SAFETY: we only read the value if the compare_exchange is successful,
        // which guarantees that no other reader or writer will be accessing it

        let ret = unsafe { (*self.value.get()).assume_init_ref().clone() };

        // restore status to be accessible
        self.status
            .store(ItemStatus::Accessible.into(), Ordering::Relaxed);

        // Write data to item
        Some(ret)
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
