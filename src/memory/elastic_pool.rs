use std::sync::{Arc, Mutex};

const INITIAL_FRAMES: usize = 3;
const SMALL_STEP: usize = 5;
const LARGE_STEP: usize = 10;
const STEP_SWITCH_FRAMES: usize = 20;
const HIGH_WATERMARK: usize = 8;
const SHRINK_RELEASE_STREAK: usize = 10;

#[derive(Debug, Clone, Copy)]
pub struct PoolStats {
    pub total_frames: usize,
    pub free_frames: usize,
    pub expand_count: usize,
    pub shrink_count: usize,
    pub acquire_count: usize,
    pub alloc_count: usize,
}

struct Group {
    size: usize,
    borrowed: usize,
    free: Vec<Vec<u8>>, // LIFO within group
}

impl Group {
    fn new(size: usize, frame_bytes: usize) -> Self {
        let mut free = Vec::with_capacity(size);
        for _ in 0..size {
            free.push(vec![0u8; frame_bytes]);
        }
        Self {
            size,
            borrowed: 0,
            free,
        }
    }

    fn is_fully_free(&self) -> bool {
        self.borrowed == 0 && self.free.len() == self.size
    }
}

struct State {
    groups: Vec<Group>,
    total_frames: usize,
    release_streak: usize,
    expand_count: usize,
    shrink_count: usize,
    acquire_count: usize,
    alloc_count: usize,
}

impl State {
    fn free_frames(&self) -> usize {
        self.groups.iter().map(|g| g.free.len()).sum()
    }

    fn current_step(&self) -> usize {
        if self.total_frames < STEP_SWITCH_FRAMES {
            SMALL_STEP
        } else {
            LARGE_STEP
        }
    }

    fn low_watermark(&self) -> usize {
        let step = self.current_step();
        (step * 2).div_ceil(5).max(2) // ceil(step * 0.4)
    }

    fn append_group(&mut self, size: usize, frame_bytes: usize) {
        self.groups.push(Group::new(size, frame_bytes));
        self.total_frames += size;
        self.expand_count += 1;
        self.alloc_count += size;
    }
}

pub struct ElasticBufferPool {
    frame_bytes: usize,
    state: Mutex<State>,
}

impl ElasticBufferPool {
    pub fn new(frame_bytes: usize) -> Arc<Self> {
        let state = State {
            groups: vec![Group::new(INITIAL_FRAMES, frame_bytes)],
            total_frames: INITIAL_FRAMES,
            release_streak: 0,
            expand_count: 0,
            shrink_count: 0,
            acquire_count: 0,
            alloc_count: INITIAL_FRAMES,
        };
        Arc::new(Self {
            frame_bytes,
            state: Mutex::new(state),
        })
    }

    pub fn acquire(self: &Arc<Self>) -> PooledBuffer {
        let mut state = self.state.lock().expect("pool mutex poisoned");
        state.acquire_count += 1;

        if state.free_frames() < state.low_watermark() {
            let step = state.current_step();
            state.append_group(step, self.frame_bytes);
        }

        for (idx, group) in state.groups.iter_mut().enumerate().rev() {
            if let Some(data) = group.free.pop() {
                group.borrowed += 1;
                return PooledBuffer {
                    data: Some(data),
                    group_idx: idx,
                    pool: Arc::clone(self),
                };
            }
        }

        // Defensive fallback (should not happen): allocate one frame immediately.
        let fallback = vec![0u8; self.frame_bytes];
        state.alloc_count += 1;
        PooledBuffer {
            data: Some(fallback),
            group_idx: 0,
            pool: Arc::clone(self),
        }
    }

    pub fn stats(&self) -> PoolStats {
        let state = self.state.lock().expect("pool mutex poisoned");
        PoolStats {
            total_frames: state.total_frames,
            free_frames: state.free_frames(),
            expand_count: state.expand_count,
            shrink_count: state.shrink_count,
            acquire_count: state.acquire_count,
            alloc_count: state.alloc_count,
        }
    }

    fn release_inner(&self, group_idx: usize, mut data: Vec<u8>) {
        if data.len() != self.frame_bytes {
            data.resize(self.frame_bytes, 0);
        }

        let mut state = self.state.lock().expect("pool mutex poisoned");
        if let Some(group) = state.groups.get_mut(group_idx) {
            if group.borrowed > 0 {
                group.borrowed -= 1;
            }
            group.free.push(data);
        }

        let can_shrink = state.free_frames() >= HIGH_WATERMARK
            && state.groups.len() > 1
            && state
                .groups
                .last()
                .map(|g| g.is_fully_free())
                .unwrap_or(false)
            && state.total_frames > INITIAL_FRAMES;

        if can_shrink {
            state.release_streak += 1;
            if state.release_streak >= SHRINK_RELEASE_STREAK {
                if let Some(last) = state.groups.last() {
                    state.total_frames = state.total_frames.saturating_sub(last.size);
                }
                let _ = state.groups.pop();
                state.shrink_count += 1;
                state.release_streak = 0;
            }
        } else {
            state.release_streak = 0;
        }
    }
}

pub struct PooledBuffer {
    data: Option<Vec<u8>>,
    group_idx: usize,
    pool: Arc<ElasticBufferPool>,
}

impl PooledBuffer {
    pub fn as_slice(&self) -> &[u8] {
        self.data.as_deref().unwrap_or(&[])
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.data.as_deref_mut().unwrap_or(&mut [])
    }

    pub fn into_vec(mut self) -> Vec<u8> {
        self.data.take().unwrap_or_default()
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        if let Some(data) = self.data.take() {
            self.pool.release_inner(self.group_idx, data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_capacity() {
        let pool = ElasticBufferPool::new(1024);
        let stats = pool.stats();
        assert_eq!(stats.total_frames, INITIAL_FRAMES);
        assert_eq!(stats.free_frames, INITIAL_FRAMES);
    }

    #[test]
    fn test_expand_on_low_watermark() {
        let pool = ElasticBufferPool::new(1024);
        let _a = pool.acquire();
        let _b = pool.acquire();
        // Third acquire triggers low watermark expansion before take.
        let _c = pool.acquire();
        let stats = pool.stats();
        assert!(stats.total_frames >= INITIAL_FRAMES + SMALL_STEP);
        assert!(stats.expand_count >= 1);
    }
}
