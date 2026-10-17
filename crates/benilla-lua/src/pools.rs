//! The client's block pools. Its Lua state allocates every block of up to 256 bytes from one of
//! six fixed-size pools (`luaM_reallocPool`, `0x6fae90`), and the threshold a collection leaves
//! is capped by the least free space among them (`0x6fae00`). benilla's objects live in Rust's
//! allocator, so it keeps only the pools' book: the blocks in use and the chunks taken in each
//! pool, from the blocks the collector counts, each at its 32-bit 5.0 size.
//!
//! The book is the client's allocator run over benilla's own sequence of blocks, which a
//! collection recounts from what lives. Deviation: that sequence is not wholly the client's. A
//! table's node vector is sized as 5.1 sizes it, neither the constant table each function's parse
//! makes (`open_func`'s `luaH_new`, `0x6fcad0`) nor `lua_load`'s token buffer is counted, and a
//! prototype's vectors are grown one after another where the client's parser grows them
//! together; each moves a pool's chunk boundary, and so its free space, by what it changes.

/// The six pools' block sizes, smallest first (`0x811d34`..`0x811d48`).
pub(crate) const BLOCK_SIZES: [u32; 6] = [0x10, 0x20, 0x40, 0x60, 0x80, 0x100];

/// A pool takes its blocks 128 KB at a time: `0x20000 / size` blocks a chunk (`0x6faf1b`).
const CHUNK_BYTES: u32 = 0x20000;

/// Where a block the collector counts lives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Block {
    /// No block: a vector of no elements.
    #[default]
    None,
    /// A block of pool `i` of [`BLOCK_SIZES`].
    Pool(u8),
    /// The system heap, past the largest pool (`SMemAlloc`, `0x6fb1bb`).
    System,
}

/// Blocks in use and chunks taken, each pool.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Pools {
    used: [u32; 6],
    /// A pool takes a chunk when every block it has is in use (`0x6fb22b`), and gives none back
    /// while the state lives: a freed block goes on its chunk's free list (`0x6fafbe`), and only
    /// closing the state frees the chunks (`0x6fb2f0`).
    chunks: [u32; 6],
}

/// Blocks a chunk of pool `i` holds.
const fn per_chunk(i: usize) -> u32 {
    CHUNK_BYTES / BLOCK_SIZES[i]
}

impl Pools {
    /// Where a new block of `size` bytes goes: the first pool whose blocks hold it (`0x6fb1a3`),
    /// else the system heap.
    pub(crate) fn place(size: usize) -> Block {
        if size == 0 {
            return Block::None;
        }
        match BLOCK_SIZES.iter().position(|&b| size <= b as usize) {
            Some(i) => Block::Pool(i as u8),
            None => Block::System,
        }
    }

    /// A new block of `size` bytes (`0x6fb197`).
    pub(crate) fn alloc(&mut self, size: usize) -> Block {
        let b = Self::place(size);
        if let Block::Pool(i) = b {
            self.take(i as usize);
        }
        b
    }

    /// `0x6fb1f0`: a block from pool `i`, which takes a chunk when it has no free block.
    fn take(&mut self, i: usize) {
        if self.used[i] >= self.chunks[i] * per_chunk(i) {
            self.chunks[i] += 1;
        }
        self.used[i] += 1;
    }

    /// The block `b` given back (`0x6fafbe`).
    pub(crate) fn free(&mut self, b: Block) {
        if let Block::Pool(i) = b {
            let u = &mut self.used[i as usize];
            debug_assert!(*u > 0, "a block freed from pool {i}, which has none in use");
            *u = u.saturating_sub(1);
        }
    }

    /// `0x6fae90` with a block: a block whose pool holds the new size stays (`0x6fb052`), a larger
    /// size moves it to the first pool past its own that holds it (`0x6fb070`), else to the system
    /// heap (`0x6fb094`), the new block taken before the old is given back; a system block stays
    /// there at any size (`0x6fb03b`); size 0 frees it.
    pub(crate) fn realloc(&mut self, b: Block, size: usize) -> Block {
        match b {
            Block::None => self.alloc(size),
            _ if size == 0 => {
                self.free(b);
                Block::None
            }
            Block::System => Block::System,
            Block::Pool(i) if size <= BLOCK_SIZES[i as usize] as usize => b,
            Block::Pool(_) => {
                let nb = self.alloc(size);
                self.free(b);
                nb
            }
        }
    }

    /// A vector 5.0's parser grows one element at a time and `close_func` then fits to `count`
    /// elements of `elem` bytes (`0x6fcb48`..`0x6fcc26`): `luaM_growaux` takes 4 first and doubles
    /// after (`0x6fc90e`..`0x6fc919`), each step a reallocation.
    pub(crate) fn grow_vector(&mut self, elem: usize, count: usize) -> Block {
        let mut b = Block::None;
        let mut cap = 0;
        while cap < count {
            cap = if cap == 0 { 4 } else { cap * 2 };
            b = self.realloc(b, cap * elem);
        }
        self.realloc(b, count * elem)
    }

    /// The least free space among the six pools (`0x6fae23`..`0x6fae65`): each pool's free blocks
    /// across its chunks times its block size. A pool that has taken no chunk has none.
    pub(crate) fn least_free(&self) -> u32 {
        (0..6)
            .map(|i| (self.chunks[i] * per_chunk(i) - self.used[i]) * BLOCK_SIZES[i])
            .min()
            .unwrap_or(0)
    }

    /// The blocks in use, as a collection's tally of what lives found them; the chunks every
    /// pool has taken stay.
    pub(crate) fn recounted(&mut self, tally: Tally) {
        for i in 0..6 {
            let need = tally.used[i].div_ceil(per_chunk(i));
            self.chunks[i] = self.chunks[i].max(need);
        }
        self.used = tally.used;
    }

    #[cfg(test)]
    pub(crate) fn used(&self) -> [u32; 6] {
        self.used
    }

    #[cfg(test)]
    pub(crate) fn chunks(&self) -> [u32; 6] {
        self.chunks
    }
}

/// The blocks in use, each pool, as a collection counts them from what lives
/// ([`Pools::recounted`]).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Tally {
    used: [u32; 6],
}

impl Tally {
    pub(crate) fn count(&mut self, b: Block) {
        if let Block::Pool(i) = b {
            self.used[i as usize] += 1;
        }
    }

    /// A block of `size` bytes made at that size and never reallocated: where [`Pools::place`]
    /// puts it.
    pub(crate) fn count_size(&mut self, size: usize) {
        self.count(Pools::place(size));
    }
}

/// Where [`Pools::grow_vector`] leaves a vector of `count` elements of `elem` bytes: the place of
/// the last capacity its growth reached, which fitting it smaller never moves.
pub(crate) fn vector_block(elem: usize, count: usize) -> Block {
    if count == 0 {
        return Block::None;
    }
    // `luaM_growaux`'s capacities, 4 and its doublings, are the powers of two from 4.
    Pools::place(count.max(4).next_power_of_two() * elem)
}

/// The threshold a collection leaves (`0x6fae00`): the count plus the larger of a quarter of it
/// and the old headroom (the threshold less the count, in 32 bits), the headroom capped by the
/// least free space among the pools.
pub(crate) fn next_threshold(old: u32, count: u32, least_free: u32) -> u32 {
    let headroom = old.wrapping_sub(count).min(least_free);
    count.wrapping_add(headroom.max(count >> 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each size goes to the first pool that holds it, past 256 bytes to the system heap.
    #[test]
    fn a_block_goes_to_the_first_pool_that_holds_it() {
        assert_eq!(Pools::place(0), Block::None);
        assert_eq!(Pools::place(1), Block::Pool(0));
        assert_eq!(Pools::place(16), Block::Pool(0));
        assert_eq!(Pools::place(17), Block::Pool(1));
        assert_eq!(Pools::place(72), Block::Pool(3));
        assert_eq!(Pools::place(256), Block::Pool(5));
        assert_eq!(Pools::place(257), Block::System);
    }

    /// A pool takes a 128 KB chunk only when its blocks are all in use, and keeps it.
    #[test]
    fn a_pool_takes_a_chunk_when_full_and_keeps_it() {
        let mut p = Pools::default();
        assert_eq!(p.least_free(), 0, "no pool has a chunk");
        // 4096 blocks of 32 bytes fill one chunk; the 4097th takes a second.
        let blocks: Vec<Block> = (0..4097).map(|_| p.alloc(17)).collect();
        assert_eq!(p.chunks()[1], 2);
        assert_eq!(p.used()[1], 4097);
        for b in blocks {
            p.free(b);
        }
        assert_eq!(p.chunks()[1], 2, "freeing gives no chunk back");
        assert_eq!(p.used()[1], 0);
        // The other five pools have no chunk, so the least free space is still none.
        assert_eq!(p.least_free(), 0);
        for size in [16, 64, 96, 128, 256] {
            p.alloc(size);
        }
        // Free: pool 0 8191 blocks of 16 bytes (131 056), 1 8192 of 32 (262 144), 2 2047 of 64
        // (131 008), 3 1364 of 96 (130 944, 1365 a chunk), 4 1023 of 128 (130 944), 5 511 of 256
        // (130 816), the least.
        assert_eq!(p.least_free(), 511 * 256);
    }

    /// A growing block moves up the pools and past them; a shrinking one stays where it is, and a
    /// system block never comes back.
    #[test]
    fn a_reallocated_block_moves_only_up() {
        let mut p = Pools::default();
        let b = p.alloc(16 * 8);
        assert_eq!(b, Block::Pool(4));
        let b = p.realloc(b, 16 * 4);
        assert_eq!(b, Block::Pool(4), "a smaller size keeps the block");
        let b = p.realloc(b, 16 * 16);
        assert_eq!(b, Block::Pool(5));
        assert_eq!(p.used(), [0, 0, 0, 0, 0, 1]);
        let b = p.realloc(b, 16 * 32);
        assert_eq!(b, Block::System);
        assert_eq!(p.used(), [0; 6]);
        assert_eq!(
            p.realloc(b, 16),
            Block::System,
            "a system block stays there"
        );
        let b = p.alloc(64);
        assert_eq!(p.realloc(b, 0), Block::None);
        assert_eq!(p.used(), [0; 6]);
    }

    /// A vector of 10 instructions grows 16, 32, 64 bytes and is fitted to 40 in its 64-byte
    /// block; one of 100 grows past 256 bytes and stays on the system heap.
    #[test]
    fn a_parser_vector_lives_where_its_growth_left_it() {
        let mut p = Pools::default();
        assert_eq!(p.grow_vector(4, 10), Block::Pool(2));
        assert_eq!(p.used(), [0, 0, 1, 0, 0, 0]);
        assert_eq!(
            p.chunks(),
            [1, 1, 1, 0, 0, 0],
            "each step took its pool's first chunk"
        );
        assert_eq!(p.grow_vector(4, 100), Block::System);
        assert_eq!(p.grow_vector(16, 0), Block::None);
        for elem in [4, 12, 16] {
            for count in 0..100 {
                assert_eq!(
                    vector_block(elem, count),
                    Pools::default().grow_vector(elem, count),
                    "{elem} x {count}"
                );
            }
        }
    }

    /// A block given back that the book never took is drift, which a test build catches.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "has none in use")]
    fn a_free_the_book_never_took_is_caught() {
        Pools::default().free(Block::Pool(2));
    }

    /// `0x6fae00` on hand-worked numbers.
    #[test]
    fn the_next_threshold_is_the_count_and_a_capped_headroom_or_a_quarter() {
        // Count 200 000 under 262 144: headroom 62 144 against a quarter, 50 000. Pools with
        // 100 000 free leave the headroom; with 30 000 free the quarter wins.
        assert_eq!(next_threshold(262_144, 200_000, 100_000), 262_144);
        assert_eq!(next_threshold(262_144, 200_000, 30_000), 250_000);
        // A pool with nothing free leaves the quarter.
        assert_eq!(next_threshold(262_144, 200_000, 0), 250_000);
        // A threshold set under the count (`collectgarbage(0)`) wraps to a headroom near 4 GB,
        // which only the pools cap.
        assert_eq!(next_threshold(0, 200_000, 120_000), 320_000);
        assert_eq!(next_threshold(0, 200_000, 10_000), 250_000);
    }
}
