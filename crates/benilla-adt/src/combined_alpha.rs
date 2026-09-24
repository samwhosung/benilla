//! MCAL → one 64×64 RGBA alpha map (R/G/B = the opacity of texture layers 1/2/3; A = 255). Ported
//! from wow-adt's `CombinedAlphaMap`: layers are ingested in MCLY order (skipping the opaque base),
//! each decoded into the next channel; encodings are 4-bit packed (×17 — `n / 15`, what the
//! reference's RGBA4444 texel reads as; wow-adt's ×16 topped out at 240), 8-bit raw, or Blizzard RLE (token bit 7 = fill, else copy; low 7 bits = count). `fix_alpha`
//! reconstructs a 64×64 plane from 63×63 source by duplicating the previous pixel at the last row/col.

use crate::McnkChunk;

/// Assembles the per-layer MCAL alpha maps into one 64×64 RGBA buffer (`y`-major, then `x`, then
/// channel R,G,B,A).
pub struct CombinedAlphaMap {
    map: Vec<u8>, // 64*64*4, [y][x][rgba]
    x: usize,
    y: usize,
    layer: usize,
    has_big_alpha: bool,
    fix_alpha: bool,
}

const W: usize = 64;

impl CombinedAlphaMap {
    /// Construct and ingest `chunk`'s alpha layers. `has_big_alpha`/`fix_alpha` select the uncompressed
    /// encoding width and the 63→64 edge fix (vanilla: `false`, `true`).
    pub fn new(chunk: &McnkChunk, has_big_alpha: bool, fix_alpha: bool) -> Self {
        let mut map = vec![0u8; W * W * 4];
        for px in map.as_chunks_mut::<4>().0 {
            px[3] = 255; // A unused, set opaque for tool visibility (matches wow-adt)
        }
        let mut s = Self {
            map,
            x: 0,
            y: 0,
            layer: 0,
            has_big_alpha,
            fix_alpha,
        };
        s.ingest_chunk_layers(chunk);
        s
    }

    /// The combined RGBA bytes (64×64×4).
    pub fn as_slice(&self) -> &[u8] {
        &self.map
    }

    fn ingest_chunk_layers(&mut self, chunk: &McnkChunk) {
        let (Some(mcly), Some(mcal)) = (&chunk.layers, &chunk.alpha) else {
            return;
        };
        for layer in mcly.layers.iter().skip(1) {
            let offset = layer.offset_in_mcal as usize;
            if layer.flags.alpha_map_compressed() {
                self.ingest_compressed(&mcal.data, offset);
            } else if self.has_big_alpha {
                self.ingest_big(&mcal.data, offset);
            } else {
                self.ingest_small(&mcal.data, offset);
            }
        }
    }

    fn ingest_big(&mut self, raw: &[u8], offset: usize) {
        const LAYER: usize = W * W;
        if offset + LAYER <= raw.len() {
            for &a in &raw[offset..offset + LAYER] {
                if !self.set_next(a) {
                    break;
                }
            }
        }
        self.next_layer();
    }

    fn ingest_small(&mut self, raw: &[u8], offset: usize) {
        const PACKED: usize = W * W / 2;
        if offset + PACKED <= raw.len() {
            for &p in &raw[offset..offset + PACKED] {
                if !self.set_next((p & 0x0F) * 17) {
                    break;
                }
                if !self.set_next(((p >> 4) & 0x0F) * 17) {
                    break;
                }
            }
        }
        self.next_layer();
    }

    fn ingest_compressed(&mut self, raw: &[u8], mut offset: usize) {
        const TARGET: usize = W * W;
        let mut out = Vec::with_capacity(TARGET);
        while out.len() < TARGET {
            if offset >= raw.len() {
                break;
            }
            let token = raw[offset];
            offset += 1;
            let count = (token & 0x7F) as usize;
            if count == 0 {
                continue;
            }
            if token & 0x80 != 0 {
                // fill
                if offset >= raw.len() {
                    break;
                }
                let v = raw[offset];
                offset += 1;
                for _ in 0..count {
                    if out.len() >= TARGET {
                        break;
                    }
                    out.push(v);
                }
            } else {
                // copy
                for _ in 0..count {
                    if offset >= raw.len() || out.len() >= TARGET {
                        break;
                    }
                    out.push(raw[offset]);
                    offset += 1;
                }
            }
        }
        out.resize(TARGET, 0);
        for &a in &out {
            if !self.set_next(a) {
                break;
            }
        }
        self.next_layer();
    }

    fn get(&self, x: usize, y: usize, layer: usize) -> u8 {
        if x < W && y < W && layer < 4 {
            self.map[(y * W + x) * 4 + layer]
        } else {
            0
        }
    }

    fn set(&mut self, x: usize, y: usize, layer: usize, a: u8) {
        if x < W && y < W && layer < 4 {
            self.map[(y * W + x) * 4 + layer] = a;
        }
    }

    fn set_next(&mut self, mut a: u8) -> bool {
        if self.fix_alpha {
            if self.x == 63 {
                a = self.get(self.x - 1, self.y, self.layer);
            }
            if self.y == 63 {
                a = self.get(self.x, self.y - 1, self.layer);
            }
        }
        self.set(self.x, self.y, self.layer, a);
        self.x += 1;
        if self.x >= W {
            self.x = 0;
            self.y += 1;
        }
        self.y < W
    }

    fn next_layer(&mut self) {
        self.layer += 1;
        self.x = 0;
        self.y = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank() -> CombinedAlphaMap {
        CombinedAlphaMap {
            map: vec![0; W * W * 4],
            x: 0,
            y: 0,
            layer: 0,
            has_big_alpha: false,
            fix_alpha: true,
        }
    }

    /// The reference packs a 4-bit layer weight into an RGBA4444 texel (the 64×64 and 32×32
    /// packers `0x6b03d0`/`0x6b08d0`), and a 4-bit unorm channel reads as `n / 15` — so a fully
    /// painted nibble is full coverage. `n × 16` stopped at 240: every "fully painted" road or rock
    /// let 6% of the layer beneath show through.
    #[test]
    fn four_bit_alpha_reads_as_n_over_15() {
        let mut m = blank();
        // Low nibble first: texel 0 = 0xF, texel 1 = 0x8.
        m.ingest_small(&[0x8F; W * W / 2], 0);
        assert_eq!(m.get(0, 0, 0), 255, "nibble 15 is full coverage");
        assert_eq!(m.get(1, 0, 0), 136, "nibble 8 is 8/15");
        let mut z = blank();
        z.ingest_small(&[0x00; W * W / 2], 0);
        assert_eq!(z.get(0, 0, 0), 0);
    }
}
