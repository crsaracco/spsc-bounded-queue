use core::{mem, ptr};
use core::alloc::Layout;
use std::alloc;
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use cache_padded::CachePadded;

pub(crate) struct ConsumerMetadata {
    pub(crate) head: AtomicUsize,
    pub(crate) shadow_tail: Cell<usize>,
}

pub(crate) struct ProducerMetadata {
    pub(crate) tail: AtomicUsize,
    pub(crate) shadow_head: Cell<usize>,
}

/// The internal memory buffer used by the queue.
///
/// Buffer holds a pointer to allocated memory which represents the bounded
/// ring buffer, as well as a head and tail atomicUsize which the producer and consumer
/// use to track location in the ring.
#[repr(C)]
pub(crate) struct Buffer<T> {
    /// A pointer to the allocated ring buffer
    pub(crate) buffer: *mut T,

    /// The bounded size as specified by the user.  If the queue reaches capacity, it will block
    /// until values are popped off.
    pub(crate) capacity: usize,

    /// The allocated size of the ring buffer, in terms of number of values (not physical memory).
    /// This will be the next power of two larger than `capacity`
    pub(crate) allocated_size: CachePadded<usize>,

    /// Consumer cacheline:
    pub(crate) consumer_metadata: CachePadded<ConsumerMetadata>,

    /// Producer cacheline:
    pub(crate) producer_metadata: CachePadded<ProducerMetadata>,
}

// The buffer itself should be Send and Sync so that we can share the two endpoints between threads.
unsafe impl<T: Send> Send for Buffer<T> {}
unsafe impl<T: Sync> Sync for Buffer<T> {}

impl<T> Buffer<T> {
    /// Attempt to pop a value off the buffer.
    ///
    /// If the buffer is empty, this method will not block.  Instead, it will return `None`
    /// signifying the buffer was empty.  The caller may then decide what to do next (e.g. spin-wait,
    /// sleep, process something else, etc)
    ///
    /// # Examples
    ///
    /// ```
    /// // Attempt to pop off a value
    /// let t = buffer.try_pop();
    /// match t {
    ///   Some(v) => {}, // Got a value
    ///   None => {}     // Buffer empty, try again later
    /// }
    /// ```
    pub fn try_pop(&self) -> Option<T> {
        let current_head = self.consumer_metadata.head.load(Ordering::Relaxed);

        if current_head == self.consumer_metadata.shadow_tail.get() {
            self.consumer_metadata.shadow_tail.set(self.producer_metadata.tail.load(Ordering::Acquire));
            if current_head == self.consumer_metadata.shadow_tail.get() {
                return None;
            }
        }

        let v = unsafe { ptr::read(self.load(current_head)) };
        self.consumer_metadata.head
            .store(current_head.wrapping_add(1), Ordering::Release);
        Some(v)
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
    pub fn skip_n(&self, n: usize) -> usize {
        let current_head = self.consumer_metadata.head.load(Ordering::Relaxed);

        self.consumer_metadata.shadow_tail.set(self.producer_metadata.tail.load(Ordering::Acquire));
        if current_head == self.consumer_metadata.shadow_tail.get() {
            return 0;
        }
        let mut diff = self.consumer_metadata.shadow_tail.get().wrapping_sub(current_head);
        if diff > n {
            diff = n
        }
        self.consumer_metadata.head
            .store(current_head.wrapping_add(diff), Ordering::Release);
        diff
    }

    /// Pop a value off the buffer.
    ///
    /// This method will block until the buffer is non-empty.  The waiting strategy is a simple
    /// spin-wait and will repeatedly call `try_pop()` until a value is available.  If you do not
    /// want a spin-wait burning CPU, you should call `try_pop()` directly and implement a different
    /// waiting strategy.
    ///
    /// # Examples
    ///
    /// ```
    /// // Block until a value is ready
    /// let t = buffer.pop();
    /// ```
    pub fn pop(&self) -> T {
        loop {
            match self.try_pop() {
                None => {}
                Some(v) => return v,
            }
        }
    }

    /// Attempt to push a value onto the buffer.
    ///
    /// If the buffer is full, this method will not block.  Instead, it will return `Some(v)`, where
    /// `v` was the value attempting to be pushed onto the buffer.  If the value was successfully
    /// pushed onto the buffer, `None` will be returned signifying success.
    ///
    /// # Examples
    ///
    /// ```
    /// // Attempt to push a value onto the buffer
    /// let t = buffer.try_push(123);
    /// match t {
    ///   Some(v) => {}, // Buffer was full, try again later
    ///   None => {}     // Value was successfully pushed onto the buffer
    /// }
    /// ```
    pub fn try_push(&self, v: T) -> Option<T> {
        let current_tail = self.producer_metadata.tail.load(Ordering::Relaxed);

        if self.producer_metadata.shadow_head.get() + self.capacity <= current_tail {
            self.producer_metadata.shadow_head.set(self.consumer_metadata.head.load(Ordering::Relaxed));
            if self.producer_metadata.shadow_head.get() + self.capacity <= current_tail {
                return Some(v);
            }
        }

        unsafe {
            self.store(current_tail, v);
        }
        self.producer_metadata.tail
            .store(current_tail.wrapping_add(1), Ordering::Release);
        None
    }

    /// Push a value onto the buffer.
    ///
    /// This method will block until the buffer is non-full.  The waiting strategy is a simple
    /// spin-wait and will repeatedly call `try_push()` until the value can be added.  If you do not
    /// want a spin-wait burning CPU, you should call `try_push()` directly and implement a different
    /// waiting strategy.
    ///
    /// # Examples
    ///
    /// ```
    /// // Block until we can push this value onto the buffer
    /// buffer.try_push(123);
    /// ```
    pub fn push(&self, v: T) {
        let mut t = v;
        loop {
            match self.try_push(t) {
                Some(rv) => t = rv,
                None => return,
            }
        }
    }

    /// Load a value out of the buffer
    ///
    /// # Safety
    ///
    /// This method assumes the caller has:
    /// - Initialized a valid block of memory
    /// - Specified an index position that contains valid data
    ///
    /// The caller can use either absolute or monotonically increasing index positions, since
    /// buffer wrapping is handled inside the method.
    #[inline]
    unsafe fn load(&self, pos: usize) -> &T {
        &*self.buffer
            .offset((pos & (self.allocated_size.into_inner() - 1)) as isize)
    }

    /// Store a value in the buffer
    ///
    /// # Safety
    ///
    /// This method assumes the caller has:
    /// - Initialized a valid block of memory
    #[inline]
    unsafe fn store(&self, pos: usize, v: T) {
        let end = self.buffer
            .offset((pos & (self.allocated_size.into_inner() - 1)) as isize);
        ptr::write(&mut *end, v);
    }
}

/// Handles deallocation of heap memory when the buffer is dropped
impl<T> Drop for Buffer<T> {
    fn drop(&mut self) {
        // Pop the rest of the values off the queue.  By moving them into this scope,
        // we implicitly call their destructor

        // TODO this could be optimized to avoid the atomic operations / book-keeping...but
        // since this is the destructor, there shouldn't be any contention... so meh?
        while let Some(_) = self.try_pop() {}

        if mem::size_of::<T>() > 0 {
            unsafe {
                let layout = Layout::from_size_align(
                    self.allocated_size.into_inner() * mem::size_of::<T>(),
                    mem::align_of::<T>(),
                ).unwrap();
                alloc::dealloc(self.buffer as *mut u8, layout);
            }
        }
    }
}

/// Allocates a memory buffer on the heap and returns a pointer to it
pub(crate) unsafe fn allocate_buffer<T>(capacity: usize) -> *mut T {
    let adjusted_size = capacity.next_power_of_two();
    let size = adjusted_size
        .checked_mul(mem::size_of::<T>())
        .expect("capacity overflow");

    let layout = Layout::from_size_align(size, mem::align_of::<T>()).unwrap();

    let ptr = if size > 0 {
        alloc::alloc(layout) as *mut T
    } else {
        mem::align_of::<T>() as *mut T
    };

    if ptr.is_null() {
        alloc::handle_alloc_error(layout)
    } else {
        ptr
    }
}