use std::cell::Cell;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::usize;
use std::marker::PhantomData;

use cache_padded::CachePadded;

mod buffer;
use buffer::{Buffer, allocate_buffer};

mod producer;
pub use producer::Producer;

mod consumer;
pub use consumer::Consumer;

/// Creates a new SPSC Queue, returning a Producer and Consumer handle.
///
/// Capacity specifies the size of the bounded queue to create.  Actual memory usage
/// will be `capacity.next_power_of_two() * size_of::<T>()`, since ringbuffers with
/// power of two sizes are more efficient to operate on (can use a bitwise AND to index
/// into the ring instead of a more expensive modulo operator).
///
/// # Examples
///
/// Here is a simple usage of make, using the queue within the same thread:
///
/// ```
/// // Create a queue with capacity to hold 100 values
/// let (p, c) = make(100);
///
/// // Push `123` onto the queue
/// p.push(123);
///
/// // Pop the value back off
/// let t = c.pop();
/// assert!(t == 123);
/// ```
///
/// Of course, a SPSC queue is really only useful if you plan to use it in a multi-threaded
/// environment.  The Producer and Consumer can both be sent to a thread, providing a fast, bounded
/// one-way communication channel between those threads:
///
/// ```
/// use std::thread;
///
/// let (p, c) = make(500);
///
/// // Spawn a new thread and move the Producer into it
/// thread::spawn(move|| {
///   for i in 0..100000 {
///     p.push(i as u32);
///   }
/// });
///
/// // Back in the first thread, start Pop'ing values off the queue
/// for i in 0..100000 {
///   let t = c.pop();
///   assert!(t == i);
/// }
///
/// ```
///
/// # Panics
///
/// If the requested queue size is larger than available memory (e.g.
/// `capacity.next_power_of_two() * size_of::<T>() > available memory` ), this function will abort
/// with an OOM panic.
pub fn make<'a, T: 'a>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let ptr = unsafe { allocate_buffer(capacity) };

    let arc = Arc::new(Buffer {
        buffer: ptr,
        capacity,
        allocated_size: CachePadded::new(capacity.next_power_of_two()),

        consumer_metadata: CachePadded::new(buffer::ConsumerMetadata{
            head: AtomicUsize::new(0),
            shadow_tail: Cell::new(0),
        }),

        producer_metadata: CachePadded::new(buffer::ProducerMetadata{
            tail: AtomicUsize::new(0),
            shadow_head: Cell::new(0),
        }),
    });

    (
        Producer {
            buffer: arc.clone(),
            _marker: PhantomData,
        },
        Consumer {
            buffer: arc.clone(),
            _marker: PhantomData,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_producer_push() {
        let (p, _) = super::make(10);

        for i in 0..9 {
            p.push(i);
            assert!(p.capacity() == 10);
            assert!(p.size() == i + 1);
        }
    }

    #[test]
    fn test_consumer_pop() {
        let (p, c) = super::make(10);

        for i in 0..9 {
            p.push(i);
            assert!(p.capacity() == 10);
            assert!(p.size() == i + 1);
        }

        for i in 0..9 {
            assert!(c.size() == 9 - i);
            let t = c.pop();
            assert!(c.capacity() == 10);
            assert!(c.size() == 9 - i - 1);
            assert!(t == i);
        }
    }

    #[test]
    fn test_consumer_skip() {
        let (p, c) = super::make(10);

        for i in 0..9 {
            p.push(i);
            assert!(p.capacity() == 10);
            assert!(p.size() == i + 1);
        }
        assert!(c.size() == 9);
        assert!(c.skip_n(5) == 5);
        assert!(c.size() == 4);
        for i in 0..4 {
            assert!(c.size() == 4 - i);
            let t = c.pop();
            assert!(c.capacity() == 10);
            assert!(c.size() == 4 - i - 1);
            assert!(t == i + 5);
        }
        assert!(c.size() == 0);
        assert!(c.skip_n(5) == 0);
    }

    #[test]
    fn test_consumer_skip_whole_buf() {
        let (p, c) = super::make(9);

        for i in 0..9 {
            p.push(i);
            assert!(p.capacity() == 9);
            assert!(p.size() == i + 1);
        }
        assert!(c.size() == 9);
        assert!(c.skip_n(9) == 9);
        assert!(c.size() == 0);
    }

    #[test]
    fn test_try_push() {
        let (p, _) = super::make(10);

        for i in 0..10 {
            p.push(i);
            assert!(p.capacity() == 10);
            assert!(p.size() == i + 1);
        }

        match p.try_push(10) {
            Some(v) => {
                assert!(v == 10);
            }
            None => assert!(false, "Queue should not have accepted another write!"),
        }
    }

    #[test]
    fn test_try_poll() {
        let (p, c) = super::make(10);

        match c.try_pop() {
            Some(_) => assert!(false, "Queue was empty but a value was read!"),
            None => {}
        }

        p.push(123);

        match c.try_pop() {
            Some(v) => assert!(v == 123),
            None => assert!(false, "Queue was not empty but poll() returned nothing!"),
        }

        match c.try_pop() {
            Some(_) => assert!(false, "Queue was empty but a value was read!"),
            None => {}
        }
    }

    #[test]
    fn test_threaded() {
        let (p, c) = super::make(500);

        thread::spawn(move || {
            for i in 0..100000 {
                p.push(i);
            }
        });

        for i in 0..100000 {
            let t = c.pop();
            assert!(t == i);
        }
    }

    extern crate time;
    use self::time::PreciseTime;
    use std::sync::mpsc::sync_channel;

    #[test]
    #[ignore]
    fn bench_spsc_throughput() {
        let iterations: i64 = 2i64.pow(14);

        let (p, c) = make(iterations as usize);

        let start = PreciseTime::now();
        for i in 0..iterations as usize {
            p.push(i);
        }
        let t = c.pop();
        assert!(t == 0);
        let end = PreciseTime::now();
        let throughput =
            (iterations as f64 / (start.to(end)).num_nanoseconds().unwrap() as f64) * 1000000000f64;
        println!(
            "Spsc Throughput: {}/s -- (iterations: {} in {} ns)",
            throughput,
            iterations,
            (start.to(end)).num_nanoseconds().unwrap()
        );
    }

    #[test]
    #[ignore]
    fn bench_chan_throughput() {
        let iterations: i64 = 2i64.pow(14);

        let (tx, rx) = sync_channel(iterations as usize);

        let start = PreciseTime::now();
        for i in 0..iterations as usize {
            tx.send(i).unwrap();
        }
        let t = rx.recv().unwrap();
        assert!(t == 0);
        let end = PreciseTime::now();
        let throughput =
            (iterations as f64 / (start.to(end)).num_nanoseconds().unwrap() as f64) * 1000000000f64;
        println!(
            "Chan Throughput: {}/s -- (iterations: {} in {} ns)",
            throughput,
            iterations,
            (start.to(end)).num_nanoseconds().unwrap()
        );
    }

}
