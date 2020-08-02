use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::marker::PhantomData;

use crate::Buffer;

/// A handle to the queue which allows consuming values from the buffer.
pub struct Consumer<T> {
    pub(crate) buffer: Arc<Buffer<T>>,
    pub(crate) _marker: PhantomData<*const T>, // un-implement Sync for Consumer
}

// Consumer should be Send so that we can send it to a different thread.
// It should **not** be Sync because this is a SPSC queue; both endpoints are each only able to be
// safely used in one thread at a time.
unsafe impl<T: Send> Send for Consumer<T> {}

impl<T> Consumer<T> {
    /// Pop a value off the queue.
    ///
    /// If the buffer contains values, this method will execute immediately and return a value.
    /// If the buffer is empty, this method will block until a value becomes available.  The
    /// waiting strategy is a simple spin-wait. If you do not want a spin-wait burning CPU, you
    /// should call `try_push()` directly and implement a different waiting strategy.
    ///
    /// # Examples
    ///
    /// ```
    /// let (_, consumer) = make(100);
    ///
    /// // Block until a value becomes available
    /// let t = consumer.pop();
    /// ```
    pub fn pop(&self) -> T {
        (*self.buffer).pop()
    }

    /// Attempt to pop a value off the queue.
    ///
    /// This method does not block.  If the queue is empty, the method will return `None`.  If
    /// there is a value available, the method will return `Some(v)`, where `v` is the value
    /// being popped off the queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use bounded_spsc_queue::*;
    ///
    /// let (_, consumer) = make(100);
    ///
    /// // Attempt to pop a value off the queue
    /// let t = consumer.try_pop();
    /// match t {
    ///     Some(v) => {},      // Successfully popped a value
    ///     None => {}          // Queue empty, try again later
    /// }
    /// ```
    pub fn try_pop(&self) -> Option<T> {
        (*self.buffer).try_pop()
    }

    /// Attempts to pop (and discard) at most `n` values off the buffer.
    ///
    /// Returns the amount of values successfully skipped.
    ///
    /// # Safety
    ///
    /// *WARNING:* This will leak at most `n` values from the buffer, i.e. the destructors of the
    /// objects skipped over will not be called. This function is intended to be used on buffers that
    /// contain non-`Drop` data, such as a `Buffer<f32>`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bounded_spsc_queue::*;
    ///
    /// let (_, consumer) = make(100);
    ///
    /// let mut read_position = 0; // current buffer index
    /// read_position += consumer.skip_n(512); // try to skip at most 512 elements
    /// ```
    pub fn skip_n(&self, n: usize) -> usize {
        (*self.buffer).skip_n(n)
    }
    /// Returns the total capacity of this queue
    ///
    /// This value represents the total capacity of the queue when it is full.  It does not
    /// represent the current usage.  For that, call `size()`.
    ///
    /// # Examples
    ///
    /// ```
    /// let (_, consumer) = make(100);
    ///
    /// assert!(consumer.capacity() == 100);
    /// let t = consumer.pop();
    /// assert!(producer.capacity() == 100);
    /// ```
    pub fn capacity(&self) -> usize {
        (*self.buffer).capacity
    }

    /// Returns the current size of the queue
    ///
    /// This value represents the current size of the queue.  This value can be from 0-`capacity`
    /// inclusive.
    ///
    /// # Examples
    ///
    /// ```
    /// let (_, consumer) = make(100);
    ///
    /// //... producer pushes somewhere ...
    ///
    /// assert!(consumer.size() == 10);
    /// consumer.pop();
    /// assert!(producer.size() == 9);
    /// ```
    pub fn size(&self) -> usize {
        (*self.buffer).producer_metadata.tail.load(Ordering::Acquire) - (*self.buffer).consumer_metadata.head.load(Ordering::Acquire)
    }
}