use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use rbuf::{BufferedAtomic, BufferedAtomicSeq};

fn main() {
    let write_thread_count = 2;
    let read_thread_count = 2;

    // RingBuffer parameters
    let slot_count = 16;
    let buffer_size = 128;

    let write_data = vec![0; buffer_size];

    // Counters used to calculate throughput
    let write_counter = Arc::new(AtomicUsize::new(0));
    let read_counter = Arc::new(AtomicUsize::new(0));

    // let rbuf_write = Arc::new(RingBuffer::new(buffer_size, slot_count, 0));
    let rbuf_write = Arc::new(BufferedAtomicSeq::new(slot_count));
    let rbuf_read = rbuf_write.clone();

    for _ in 0..write_thread_count {
        let write_counter = write_counter.clone();
        let rbuf_write = rbuf_write.clone();
        let write_data = write_data.clone();
        thread::spawn(move || {
            loop {
                let res = rbuf_write.write(1);
                if res.is_ok() {
                    write_counter.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
    }

    for _ in 0..read_thread_count {
        let read_counter = read_counter.clone();
        let rbuf_read = rbuf_read.clone();
        thread::spawn(move || {
            let mut out_buff = vec![0; buffer_size];
            loop {
                // rbuf_read.read_to_buf(&mut out_buff);
                let res = rbuf_read.read();
                if res.is_some() {
                    read_counter.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
    }

    let mut last_print = Instant::now();

    loop {
        if last_print.elapsed().as_secs_f64() >= 1.0 {
            last_print = Instant::now();

            let writes = write_counter.load(Ordering::Relaxed);
            let mut writes_str = format!("{}", writes);
            if writes > 1e6 as usize {
                writes_str = format!("{:.2}M", writes as f64 / 1e6_f64);
            } else if writes > 1e3 as usize {
                writes_str = format!("{:.2}K", writes as f64 / 1e3_f64);
            }

            let reads = read_counter.load(Ordering::Relaxed);
            let mut reads_str = format!("{}", reads);
            if reads > 1e6 as usize {
                reads_str = format!("{:.2}M", reads as f64 / 1e6_f64);
            } else if reads > 1e3 as usize {
                reads_str = format!("{:.2}K", reads as f64 / 1e3_f64);
            }

            println!(
                "{} writing threads, throughput: {} writes/s \n{} reading threads, throughput: {} reads/s",
                write_thread_count, writes_str, read_thread_count, reads_str
            );

            // reset counters
            write_counter.store(0, Ordering::Relaxed);
            read_counter.store(0, Ordering::Relaxed);
        }
    }
}
