use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicUsize, Ordering},
};

///! This is the core component for the buffered atomic types

/// Stores the data for the ring buffer
#[repr(align(64))]
pub struct AtomicItem<T>
where
    T: Clone,
{
    value: UnsafeCell<std::mem::MaybeUninit<T>>,
    /// Sequence
    /// uneven = writing
    /// > 0 = initialized
    pub(crate) seq: AtomicUsize,

    /// used for tests only
    #[cfg(test)]
    pub(crate) drop_count: AtomicUsize,
}

unsafe impl<T: Clone> Send for AtomicItem<T> {}
unsafe impl<T: Clone> Sync for AtomicItem<T> {}

impl<T> Default for AtomicItem<T>
where
    T: Clone,
{
    fn default() -> Self {
        Self {
            value: UnsafeCell::new(MaybeUninit::uninit()),
            seq: AtomicUsize::new(0),
            #[cfg(test)]
            drop_count: AtomicUsize::new(0),
        }
    }
}

impl<T> AtomicItem<T>
where
    T: Clone,
{
    pub fn new() -> Self {
        Self::default()
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

        // if we are about to overflow seq, skip 0 index or the value won't be dropped in the unsafe section
        let new_seq = if seq != usize::MAX - 1 { seq + 1 } else { 1 };

        // try to get write access by attempting to increment to an uneven value
        let result = self
            .seq
            .compare_exchange(seq, new_seq, Ordering::AcqRel, Ordering::Relaxed);

        // Failed to lock for writing
        if result.is_err() {
            return Err(value);
        }

        // SAFETY: we only modify the item if the compare_exchange is successful,
        // which guarantees that no other writer will be accessing it

        // If item has already been initialized we need to drop old value before writing new
        unsafe {
            // if sequence is > 0 the item has been initialized
            if seq != 0 {
                #[cfg(test)]
                self.drop_count.fetch_add(1, Ordering::SeqCst);
                // drop the value because it has already been initialized
                (*self.value.get()).assume_init_drop();
            }

            // Write data to item
            (*self.value.get()).write(value);
        }

        // Complete write by incrementing seq to an even value
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
