use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::marker::PhantomData;

use crate::Buffer;

/// A handle to the queue which allows adding values into the buffer.
pub struct Producer<T> {
    pub(crate) buffer: Arc<Buffer<T>>,
    pub(crate) _marker: PhantomData<*const T>, // un-implement Sync for Producer
}

// Producer should be Send so that we can send it to a different thread.
// It should **not** be Sync because this is a SPSC queue; both endpoints are each only able to be
// safely used in one thread at a time.
unsafe impl<T: Send> Send for Producer<T> {}

impl<T> Producer<T> {
    /// Push a value onto the buffer.
    ///
    /// If the buffer is non-full, the operation will execute immediately.  If the buffer is full,
    /// this method will block until the buffer is non-full.  The waiting strategy is a simple
    /// spin-wait. If you do not want a spin-wait burning CPU, you should call `try_push()`
    /// directly and implement a different waiting strategy.
    ///
    /// # Examples
    ///
    /// ```
    /// let (producer, _) = make(100);
    ///
    /// // Block until we can push this value onto the queue
    /// producer.push(123);
    /// ```
    pub fn push(&self, v: T) {
        (*self.buffer).push(v);
    }

    /// Attempt to push a value onto the buffer.
    ///
    /// This method does not block.  If the queue is not full, the value will be added to the
    /// queue and the method will return `None`, signifying success.  If the queue is full,
    /// this method will return `Some(v)``, where `v` is your original value.
    ///
    /// # Examples
    ///
    /// ```
    /// let (producer, _) = make(100);
    ///
    /// // Attempt to add this value to the queue
    /// match producer.try push(123) {
    ///     Some(v) => {}, // Queue full, try again later
    ///     None => {}     // Value added to queue
    /// }
    /// ```
    pub fn try_push(&self, v: T) -> Option<T> {
        (*self.buffer).try_push(v)
    }

    /// Returns the total capacity of this queue
    ///
    /// This value represents the total capacity of the queue when it is full.  It does not
    /// represent the current usage.  For that, call `size()`.
    ///
    /// # Examples
    ///
    /// ```
    /// let (producer, _) = make(100);
    ///
    /// assert!(producer.capacity() == 100);
    /// producer.push(123);
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
    /// let (producer, _) = make(100);
    ///
    /// assert!(producer.size() == 0);
    /// producer.push(123);
    /// assert!(producer.size() == 1);
    /// ```
    pub fn size(&self) -> usize {
        (*self.buffer).producer_metadata.tail.load(Ordering::Acquire) - (*self.buffer).consumer_metadata.head.load(Ordering::Acquire)
    }

    /// Returns the available space in the queue
    ///
    /// This value represents the number of items that can be pushed onto the queue before it
    /// becomes full.
    ///
    /// # Examples
    ///
    /// ```
    /// let (producer, _) = make(100);
    ///
    /// assert!(producer.free_space() == 100);
    /// producer.push(123);
    /// assert!(producer.free_space() == 99);
    /// ```
    pub fn free_space(&self) -> usize {
        self.capacity() - self.size()
    }
}