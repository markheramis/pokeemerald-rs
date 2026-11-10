//! A bounded, underrun-safe sample ring buffer: the seam between a producer
//! of rendered PCM — in practice the integration crate's frame-driven music
//! player, pushing the `audio` crate's M4A output — and
//! [`crate::audio::AudioOutput`], which drains it once per audio device
//! callback.
//!
//! [`ring_buffer`] splits into a cloneable [`Producer`] (write side) and a
//! single [`Consumer`] (read side) sharing one fixed-capacity array of
//! atomic slots. The consumer side — the real-time device callback — never
//! takes a lock at all: it only loads and stores plain atomics, so a
//! producer stalled or descheduled mid-push can never block it.
//!
//! [`Consumer::fill`] is the hot path every consumer of a ring buffer is
//! built on — the real device callback, [`crate::resample::Resampler`], and
//! the null backend used in tests all drive it, in one bulk call per
//! callback (or, for an oversized callback the resampler bounds into several
//! fixed-size chunks, one call per chunk — see `crate::resample`). It
//! bulk-drains whatever is published into the requested slice without ever
//! blocking, and pads any shortfall with silence, adding that shortfall to
//! the underrun counter in a single consolidated update.
//! [`Consumer::pop_or_silence`] is the same "fill silence and count one
//! underrun when the buffer runs dry" rule for a single sample, retained for
//! callers that genuinely want one sample at a time.
//!
//! ## Invariants
//!
//! The only state both sides share is `Shared::occupied`, an `AtomicUsize`
//! in `[0, capacity]`: the producer publishes with `fetch_add(Release)`
//! after its slot stores, the consumer frees with `fetch_sub(Release)` after
//! its slot loads, and each side reads it with `Acquire`, which orders every
//! slot access against the other side's. The consumer never takes
//! `Shared::head`'s mutex, which only serialises cloned producers, so a
//! producer stalled mid-push cannot block the callback. Each index is
//! reduced `% capacity` on every advance rather than left as an unbounded
//! counter: a capacity that does not divide `usize::MAX + 1` would alias
//! two sequence positions onto one slot the instant such a counter wrapped.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// Shared state behind a [`Producer`]/[`Consumer`] pair.
struct Shared {
    /// Fixed-capacity backing storage; slot `i % capacity` holds the bit
    /// pattern of the sample at sequence position `i`.
    buffer: Box<[AtomicU32]>,
    capacity: usize,
    /// The producer-side write index (`< capacity`), guarded by the mutex
    /// that also serializes concurrent [`Producer`] clones. Never locked by
    /// [`Consumer`] — see the module docs' "Invariants" section.
    head: Mutex<usize>,
    /// Samples published and not yet consumed; the only state the producer
    /// and consumer sides share. Always in `[0, capacity]`.
    occupied: AtomicUsize,
    underruns: AtomicU64,
    /// Samples ever published by [`Producer::push`], wrapping. Written only
    /// while `head` is held, and only after `occupied` has published those
    /// samples, so [`Producer::pushed`] never marks a push the consumer
    /// cannot see yet. [`Producer::pushed`] reads it without the lock.
    pushed: AtomicU64,
    /// Samples ever taken off the queue, read or discarded, wrapping.
    /// Written only by the consumer side, so with `pushed` it fixes the
    /// queue's position in push order: the queued samples pushed before a
    /// [`Producer::pushed`] mark number `mark - consumed`, whatever the
    /// producer pushes meanwhile. Kept here so a reclaimed consumer
    /// continues the count.
    consumed: AtomicU64,
    /// Whether a [`Consumer`] currently exists. Lets a replacement consumer be
    /// claimed after the previous one is dropped with its device stream.
    consumer_alive: AtomicBool,
    /// The discard the consumer's next read applies: [`NO_DISCARD`],
    /// [`DISCARD_ALL`], or a [`Producer::pushed`] mark before which queued
    /// samples are dropped. Shared so a producer-side `stop` can bound a
    /// discard the consumer, already moved into a device callback, still
    /// holds.
    pending_discard: AtomicU64,
}

/// [`Shared::pending_discard`]: nothing to drop at the next read.
const NO_DISCARD: u64 = u64::MAX;
/// [`Shared::pending_discard`]: drop everything queued at the next read.
const DISCARD_ALL: u64 = u64::MAX - 1;

/// Lock `head`, recovering from poisoning rather than propagating a panic.
///
/// A panic on the producer side (e.g. a bug in the mixer feeding it) should
/// not also take down the audio device callback thread. The callback never
/// takes this lock at all, so poisoning it can only ever affect other
/// producer clones — the worst it should cost is a torn-looking push, not a
/// crash.
fn lock_head(head: &Mutex<usize>) -> std::sync::MutexGuard<'_, usize> {
    head.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Create a bounded sample ring buffer, split into a producer half and a
/// consumer half.
///
/// `capacity` is in samples, not frames: for interleaved stereo, a
/// `capacity` of `2 * frames` holds `frames` frames of audio.
#[must_use]
pub fn ring_buffer(capacity: usize) -> (Producer, Consumer) {
    let buffer: Box<[AtomicU32]> = (0..capacity).map(|_| AtomicU32::new(0)).collect();
    let shared = Arc::new(Shared {
        buffer,
        capacity,
        head: Mutex::new(0),
        occupied: AtomicUsize::new(0),
        underruns: AtomicU64::new(0),
        pushed: AtomicU64::new(0),
        consumed: AtomicU64::new(0),
        consumer_alive: AtomicBool::new(true),
        pending_discard: AtomicU64::new(NO_DISCARD),
    });
    (
        Producer {
            shared: Arc::clone(&shared),
        },
        Consumer { shared, tail: 0 },
    )
}

/// The write half of a sample ring buffer.
///
/// Cheap to clone (all clones share the same underlying buffer) and
/// `Send + Sync`, which is the point: a producer may push rendered PCM here
/// from any thread while [`Consumer`] drains it from the audio device
/// callback thread.
#[derive(Clone)]
pub struct Producer {
    shared: Arc<Shared>,
}

impl Producer {
    /// Push as many of `samples` as fit; returns the number actually
    /// written.
    ///
    /// Excess samples (buffer full) are dropped, not queued or blocked on —
    /// a slow producer must never stall the audio device thread, and a full
    /// buffer already means playback has fallen behind generation.
    #[must_use = "a return value less than `samples.len()` means some samples were dropped"]
    pub fn push(&self, samples: &[f32]) -> usize {
        let mut head = lock_head(&self.shared.head);
        // Acquire: synchronizes-with the consumer's `Release` `fetch_sub`,
        // so the slots it already read are visible as free before this call
        // overwrites them. May already be stale-high (the consumer can free
        // more concurrently) — that only makes `space` an underestimate,
        // never wrong in a way that corrupts the buffer.
        let occupied = self.shared.occupied.load(Ordering::Acquire);
        let space = self.shared.capacity - occupied;
        let n = samples.len().min(space);
        for (i, &sample) in samples[..n].iter().enumerate() {
            let idx = (*head + i) % self.shared.capacity;
            self.shared.buffer[idx].store(sample.to_bits(), Ordering::Relaxed);
        }
        if n > 0 {
            *head = (*head + n) % self.shared.capacity;
            // Release: publishes the slot stores above to the consumer's
            // `Acquire` load of `occupied`. The push count follows this
            // publish, so a `stop` that reads [`Self::pushed`] in between
            // cannot mark samples the consumer cannot see — those samples
            // are after the mark and still play. Counting first let that
            // stop cover a push the discard had not yet observed, and the
            // next fill played it.
            self.shared.occupied.fetch_add(n, Ordering::Release);
            #[cfg(test)]
            tests::between_push_publish_and_count();
            let pushed = self.shared.pushed.load(Ordering::Relaxed);
            self.shared.pushed.store(
                pushed.wrapping_add(u64::try_from(n).unwrap_or(u64::MAX)),
                Ordering::Release,
            );
        }
        n
    }

    /// Samples ever published to the consumer, wrapping on overflow.
    ///
    /// A mark taken here splits the queue: [`HeldQueue::discard_pushed_before`]
    /// keeps only what was pushed after it. Used by the audio transport to
    /// record where a `stop` landed.
    ///
    /// Never takes the producer lock, so a `stop` cannot wait behind another
    /// producer's push. The count advances only after `occupied` publishes
    /// the samples, so the mark never covers a push the consumer cannot see.
    /// A push still between those two steps falls after the mark.
    pub(crate) fn pushed(&self) -> u64 {
        self.shared.pushed.load(Ordering::Acquire)
    }

    /// Narrow a pending [`Consumer::discard_queued_at_first_read`] to the
    /// samples pushed before `mark`, so what is pushed after it survives
    /// that read. Does nothing once the read has happened, when nothing is
    /// pending, or when an earlier mark already bounds it. Lock-free.
    pub(crate) fn keep_pushed_from_at_first_read(&self, mark: u64) {
        let _ = self.shared.pending_discard.compare_exchange(
            DISCARD_ALL,
            mark.min(DISCARD_ALL - 1),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Whether a [`Consumer::discard_queued_at_first_read`] is still owed:
    /// the consumer has not read since it was requested. Lock-free.
    pub(crate) fn owes_first_read_discard(&self) -> bool {
        self.shared.pending_discard.load(Ordering::Acquire) != NO_DISCARD
    }

    /// Total samples this ring holds, fixed when it was created.
    ///
    /// Distinct from [`Self::available_space`], which is this minus whatever
    /// is queued at the moment of the call. A caller asking "is the ring
    /// empty?" wants to compare against this; a snapshot of free space taken
    /// while anything was queued understates it and would read a still-queued
    /// ring as empty.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.shared.capacity
    }

    /// Samples free right now.
    #[must_use]
    pub fn available_space(&self) -> usize {
        self.shared
            .capacity
            .saturating_sub(self.shared.occupied.load(Ordering::Acquire))
    }

    /// Total samples emitted as silence so far because a consumer wanted
    /// more than was queued (see [`Consumer::pop_or_silence`]).
    #[must_use]
    pub fn underruns(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed)
    }

    /// Claim a replacement [`Consumer`] once the previous one has been
    /// dropped, keeping the queued samples and the underrun counter.
    ///
    /// Returns `None` while a consumer still exists: the one-consumer
    /// invariant is never relaxed. The replacement's read index is rebuilt
    /// from the write index and the queued count, so it resumes exactly
    /// where the dropped consumer stopped. Used by the audio transport to
    /// re-attach a rebuilt device stream to the same queue.
    pub(crate) fn reclaim_consumer(&self) -> Option<Consumer> {
        // Holding `head` excludes pushes, so the write index and queued
        // count describe one consistent state while the tail is derived.
        let head = lock_head(&self.shared.head);
        self.shared
            .consumer_alive
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        let capacity = self.shared.capacity;
        let occupied = self.shared.occupied.load(Ordering::Acquire);
        let tail = if capacity == 0 {
            0
        } else {
            (*head + capacity - occupied) % capacity
        };
        // A discard left by the previous consumer was for its stream.
        self.shared
            .pending_discard
            .store(NO_DISCARD, Ordering::Release);
        Some(Consumer {
            shared: Arc::clone(&self.shared),
            tail,
        })
    }
}

/// The read half of a sample ring buffer, driven once per audio device
/// callback (or, for the null backend, once per manual test/harness call).
///
/// Not `Clone`, and its draining methods take `&mut self`: there is exactly
/// one `Consumer`, and the borrow checker — not a runtime lock, which is the
/// whole point of this type — is what rules out two threads racing on the
/// same drain.
pub struct Consumer {
    shared: Arc<Shared>,
    /// This consumer's own read index (`< capacity`). Touched by no one
    /// else, ever — see the module docs' "Invariants" section.
    tail: usize,
}

impl Consumer {
    /// Pop the next queued sample, or `None` if the buffer is empty.
    ///
    /// A pure query: does not touch the underrun counter. Most callers want
    /// [`Consumer::pop_or_silence`] or [`Consumer::fill`] instead.
    pub fn try_pop(&mut self) -> Option<f32> {
        self.settle_pending_discard();
        // Acquire: synchronizes-with the producer's `Release` `fetch_add`,
        // so a published slot's value is visible before this reads it.
        if self.shared.occupied.load(Ordering::Acquire) == 0 {
            return None;
        }
        let idx = self.tail % self.shared.capacity;
        let sample = f32::from_bits(self.shared.buffer[idx].load(Ordering::Relaxed));
        self.tail = (self.tail + 1) % self.shared.capacity;
        self.shared.consumed.fetch_add(1, Ordering::Relaxed);
        // Release: publishes the freed slot to the producer's `Acquire`
        // load of `occupied`.
        self.shared.occupied.fetch_sub(1, Ordering::Release);
        Some(sample)
    }

    /// Pop the next queued sample; if none is queued, count one underrun
    /// sample and return silence (`0.0`).
    ///
    /// The single-sample form of the underrun-safe rule; the callback hot
    /// path uses the bulk [`Consumer::fill`] instead, but the accounting is
    /// identical.
    #[must_use]
    pub fn pop_or_silence(&mut self) -> f32 {
        self.try_pop().unwrap_or_else(|| {
            self.shared.underruns.fetch_add(1, Ordering::Relaxed);
            0.0
        })
    }

    /// Fill `out` completely without ever blocking — bulk-draining whatever
    /// is published into the front of `out` and padding any shortfall with
    /// silence.
    ///
    /// This never takes a lock, regardless of `out.len()`: it is the
    /// callback hot path, and a producer stalled mid-push must never be able
    /// to delay it (see the module docs). Any shortfall (`out` longer than
    /// what is published) counts as that many underrun samples, added to the
    /// counter in one update — identical accounting to calling
    /// [`Consumer::pop_or_silence`] once per missing sample.
    pub fn fill(&mut self, out: &mut [f32]) {
        self.settle_pending_discard();
        // Acquire: synchronizes-with the producer's `Release` `fetch_add`,
        // so every published slot this observes is visible before the reads
        // below.
        let occupied = self.shared.occupied.load(Ordering::Acquire);
        let drained = out.len().min(occupied);
        for (i, slot) in out.iter_mut().take(drained).enumerate() {
            let idx = (self.tail + i) % self.shared.capacity;
            *slot = f32::from_bits(self.shared.buffer[idx].load(Ordering::Relaxed));
        }
        for slot in &mut out[drained..] {
            *slot = 0.0;
        }
        if drained > 0 {
            self.tail = (self.tail + drained) % self.shared.capacity;
            self.shared.consumed.fetch_add(
                u64::try_from(drained).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
            // Release: publishes the freed slots to the producer's `Acquire`
            // load of `occupied`.
            self.shared.occupied.fetch_sub(drained, Ordering::Release);
        }
        let shortfall = out.len() - drained;
        if shortfall > 0 {
            self.shared.underruns.fetch_add(
                u64::try_from(shortfall).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
        }
    }

    /// Hold off every [`Producer::push`] until the returned guard drops, so
    /// a discard decided while it is held cannot drop a sample pushed after
    /// that decision: such a push waits for the guard.
    ///
    /// Takes the producer lock, so it belongs on the thread that attaches a
    /// consumer to a device stream, never in the device callback itself.
    pub(crate) fn hold_pushes(&mut self) -> HeldQueue<'_> {
        let head = lock_head(&self.shared.head);
        HeldQueue {
            head,
            tail: &mut self.tail,
            shared: &self.shared,
        }
    }

    /// Drop whatever is queued when this consumer is next read, before that
    /// read, without counting an underrun.
    ///
    /// Lock-free, so it holds once the consumer has moved into a device
    /// callback: everything the producer pushed before the stream's first
    /// callback is dropped there, however long the stream took to build.
    /// [`Producer::keep_pushed_from_at_first_read`] can narrow it before
    /// that read.
    pub(crate) fn discard_queued_at_first_read(&mut self) {
        self.shared
            .pending_discard
            .store(DISCARD_ALL, Ordering::Release);
    }

    fn settle_pending_discard(&mut self) {
        if self.shared.pending_discard.load(Ordering::Acquire) == NO_DISCARD {
            return;
        }
        // Snapshot the queue before claiming the discard. A `stop` that
        // narrows it after the claim finds nothing pending, and anything it
        // is followed by is pushed after this snapshot, so `DISCARD_ALL`
        // drops only what predates the claim. Acquire: synchronizes-with
        // the producer's `Release` `fetch_add`. Only this consumer ever
        // lowers `occupied`, so these slots stay queued until the
        // `fetch_sub` below frees them.
        let claimed = self.shared.occupied.load(Ordering::Acquire);
        #[cfg(test)]
        tests::before_discard_claim();
        let pending = self
            .shared
            .pending_discard
            .swap(NO_DISCARD, Ordering::AcqRel);
        #[cfg(test)]
        tests::after_discard_claim();
        let queued = match pending {
            NO_DISCARD => return,
            DISCARD_ALL => claimed,
            mark => {
                // The mark may postdate `claimed`, so count the queue afresh:
                // `mark - consumed` bounds it, and only this side lowers it.
                let queued = self.shared.occupied.load(Ordering::Acquire);
                #[cfg(test)]
                tests::between_bounded_discard_loads();
                stale_before(mark, self.shared.consumed.load(Ordering::Relaxed), queued)
            }
        };
        if queued == 0 {
            return;
        }
        self.tail = (self.tail + queued) % self.shared.capacity;
        self.shared
            .consumed
            .fetch_add(u64::try_from(queued).unwrap_or(u64::MAX), Ordering::Relaxed);
        // Release: publishes the freed slots to the producer's `Acquire`
        // load of `occupied`.
        self.shared.occupied.fetch_sub(queued, Ordering::Release);
    }

    /// Samples currently queued and ready to read.
    #[must_use]
    pub fn available(&self) -> usize {
        self.shared.occupied.load(Ordering::Acquire)
    }

    /// Total samples filled with silence due to underrun so far (see
    /// [`Consumer::pop_or_silence`]).
    #[must_use]
    pub fn underruns(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed)
    }
}

impl Drop for Consumer {
    fn drop(&mut self) {
        self.shared.consumer_alive.store(false, Ordering::Release);
    }
}

/// How many of `queued` samples were pushed before `mark`: the samples
/// numbered from `consumed` up to `mark` in push order, none if the
/// consumer already read past the mark, and at most the queue if the mark
/// lies past it. The counts wrap, so their difference is read as signed: a
/// mark behind the consumer is a small negative distance, never a huge
/// positive one.
fn stale_before(mark: u64, consumed: u64, queued: usize) -> usize {
    #[expect(
        clippy::cast_possible_wrap,
        reason = "reinterpreting the wrapped difference as signed is the point: a mark behind `consumed` must read as negative"
    )]
    let ahead = mark.wrapping_sub(consumed) as i64;
    if ahead <= 0 {
        return 0;
    }
    usize::try_from(ahead).unwrap_or(usize::MAX).min(queued)
}

/// A [`Consumer`] whose producer is held off; see [`Consumer::hold_pushes`].
pub(crate) struct HeldQueue<'a> {
    head: std::sync::MutexGuard<'a, usize>,
    tail: &'a mut usize,
    shared: &'a Shared,
}

impl HeldQueue<'_> {
    /// Drop every queued sample, without counting an underrun.
    pub(crate) fn discard_all(&mut self) {
        *self.tail = *self.head;
        let queued = self.shared.occupied.load(Ordering::Acquire);
        self.shared
            .consumed
            .fetch_add(u64::try_from(queued).unwrap_or(u64::MAX), Ordering::Relaxed);
        self.shared.occupied.store(0, Ordering::Release);
    }

    /// Drop the queued samples pushed before [`Producer::pushed`] returned
    /// `mark`, keeping those pushed since, without counting an underrun.
    ///
    /// Queued samples are unread and in push order, so the oldest
    /// `mark - consumed` of them are the ones pushed before the mark.
    pub(crate) fn discard_pushed_before(&mut self, mark: u64) {
        // Acquire: synchronizes-with the producer's `Release` `fetch_add`.
        let queued = self.shared.occupied.load(Ordering::Acquire);
        let stale = stale_before(mark, self.shared.consumed.load(Ordering::Relaxed), queued);
        if stale == 0 {
            return;
        }
        *self.tail = (*self.tail + stale) % self.shared.capacity;
        self.shared
            .consumed
            .fetch_add(u64::try_from(stale).unwrap_or(u64::MAX), Ordering::Relaxed);
        // Release: publishes the freed slots to the producer's `Acquire`
        // load of `occupied`.
        self.shared.occupied.fetch_sub(stale, Ordering::Release);
    }
}

#[cfg(test)]
// Tests compare PCM sample arrays for exact equality on purpose: every
// value here is the deliberate, exactly-representable output of a ring
// buffer push/fill, not the result of accumulated floating-point math.
#[allow(clippy::float_cmp)]
mod tests;
