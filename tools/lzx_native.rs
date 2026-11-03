// build_audio_image.py's lzx() in native code (libmspack lzxd semantics, as Xenia calls it), byte for byte. Wherever the
// Python raises, or its list/bytearray slicing would quietly resize on malformed data, this returns an error instead and
// the caller runs the Python, so every input still gets exactly the Python's result.

struct Bits<'a> {
    d: &'a [u8],
    p: usize,
    buf: u64,
    n: u32,
}

type Table = Option<Vec<(u16, u8)>>;

impl Bits<'_> {
    fn ensure(&mut self, k: u32) {
        while self.n < k {
            let lo = *self.d.get(self.p).unwrap_or(&0) as u64;
            let hi = *self.d.get(self.p + 1).unwrap_or(&0) as u64;
            self.p += 2;
            self.buf = (self.buf << 16) | (hi << 8) | lo;
            self.n += 16;
        }
    }

    fn read(&mut self, k: u32) -> u64 {
        if k == 0 {
            return 0;
        }
        self.ensure(k);
        self.n -= k;
        let v = self.buf >> self.n;
        self.buf &= (1u64 << self.n) - 1;
        v
    }

    fn symbol(&mut self, table: &Table) -> Result<usize, ()> {
        let table = table.as_ref().ok_or(())?; // the Python indexes None: TypeError
        self.ensure(16);
        let (sym, length) = table[((self.buf >> (self.n - 16)) & 0xFFFF) as usize];
        if length == 0 {
            return Err(()); // "bad huffman code"
        }
        self.n -= length as u32;
        self.buf &= (1u64 << self.n) - 1;
        Ok(sym as usize)
    }
}

fn huffman(lens: &[u8]) -> Result<Table, ()> {
    let mut table = vec![(0u16, 0u8); 65536];
    let mut code = 0usize;
    for length in 1..=16u32 {
        for (sym, &l) in lens.iter().enumerate() {
            if l as u32 == length {
                let span = 1usize << (16 - length);
                if (code + 1) * span > table.len() {
                    return Err(()); // over-subscribed: the Python list would grow
                }
                table[code * span..(code + 1) * span].fill((sym as u16, length as u8));
                code += 1;
            }
        }
        code <<= 1;
    }
    Ok(if lens.iter().any(|&l| l != 0) { Some(table) } else { None })
}

fn read_lengths(bits: &mut Bits, lens: &mut [u8], first: usize, last: usize) -> Result<(), ()> {
    let mut pre_lens = [0u8; 20];
    for l in pre_lens.iter_mut() {
        *l = bits.read(4) as u8;
    }
    let pre = huffman(&pre_lens)?;
    let mut x = first;
    while x < last {
        let z = bits.symbol(&pre)?;
        let (run, value) = match z {
            17 => (bits.read(4) as usize + 4, 0),
            18 => (bits.read(5) as usize + 20, 0),
            19 => {
                let run = bits.read(1) as usize + 4;
                let s = bits.symbol(&pre)?;
                (run, (lens[x] as i64 - s as i64).rem_euclid(17) as u8)
            }
            _ => (1, (lens[x] as i64 - z as i64).rem_euclid(17) as u8),
        };
        if x + run > lens.len() {
            return Err(()); // past the padding: the Python list would grow
        }
        lens[x..x + run].fill(value);
        x += run;
    }
    Ok(())
}

fn lzx(data: &[u8], out_len: usize, window_bits: u32, reference: &[u8], out: &mut [u8]) -> Result<(), ()> {
    let slots = match window_bits {
        15 => 30,
        16 => 32,
        17 => 34,
        18 => 36,
        19 => 38,
        20 => 42,
        21 => 50,
        _ => return Err(()),
    };
    let mut extra = [0u32; 51];
    let mut base = [0i64; 51];
    for i in 0..51 {
        extra[i] = (i as i64 / 2 - 1).clamp(0, 17) as u32;
    }
    for i in 1..51 {
        base[i] = base[i - 1] + (1i64 << extra[i - 1]);
    }
    let size = 1usize << window_bits;
    if reference.len() > size {
        return Err(());
    }
    let mut window = vec![0u8; size];
    window[size - reference.len()..].copy_from_slice(reference);
    let main_count = 256 + slots * 8;
    let mut main_lens = vec![0u8; main_count + 64];
    let mut length_lens = vec![0u8; 249 + 64];
    let (mut aligned, mut main, mut lengths): (Table, Table, Table) = (None, None, None);
    let mut bits = Bits { d: data, p: 0, buf: 0, n: 0 };
    let (mut r0, mut r1, mut r2) = (1i64, 1i64, 1i64);
    let (mut posn, mut frame_posn, mut written) = (0usize, 0usize, 0usize);
    let (mut block_type, mut block_length, mut block_remaining) = (0u64, 0u64, 0i64);
    let intel_size = if bits.read(1) != 0 { (bits.read(16) << 16) | bits.read(16) } else { 0 };
    while written < out_len {
        let frame_size = 0x8000.min(out_len - written);
        let mut todo = (frame_posn + frame_size) as i64 - posn as i64;
        while todo > 0 {
            if block_remaining == 0 {
                if block_type == 3 && block_length & 1 == 1 {
                    bits.p += 1;
                }
                block_type = bits.read(3);
                block_length = (bits.read(16) << 8) | bits.read(8);
                block_remaining = block_length as i64;
                if block_type == 2 {
                    let mut lens = [0u8; 8];
                    for l in lens.iter_mut() {
                        *l = bits.read(3) as u8;
                    }
                    aligned = huffman(&lens)?;
                }
                if block_type == 1 || block_type == 2 {
                    read_lengths(&mut bits, &mut main_lens, 0, 256)?;
                    read_lengths(&mut bits, &mut main_lens, 256, main_count)?;
                    main = huffman(&main_lens[..main_count])?;
                    read_lengths(&mut bits, &mut length_lens, 0, 249)?;
                    lengths = huffman(&length_lens[..249])?;
                } else if block_type == 3 {
                    if bits.n == 0 {
                        bits.ensure(16);
                    }
                    bits.n = 0;
                    bits.buf = 0;
                    let r = data.get(bits.p..bits.p + 12).ok_or(())?;
                    let word = |i: usize| u32::from_le_bytes([r[i], r[i + 1], r[i + 2], r[i + 3]]) as i64;
                    (r0, r1, r2) = (word(0), word(4), word(8));
                    bits.p += 12;
                } else {
                    return Err(()); // "bad LZX block type"
                }
            }
            let mut run = block_remaining.min(todo);
            todo -= run;
            block_remaining -= run;
            if block_type == 3 {
                let n = usize::try_from(run).map_err(|_| ())?;
                if posn + n > size {
                    return Err(());
                }
                window[posn..posn + n].copy_from_slice(data.get(bits.p..bits.p + n).ok_or(())?);
                bits.p += n;
                posn += n;
                continue;
            }
            while run > 0 {
                let element = bits.symbol(&main)?;
                if element < 256 {
                    *window.get_mut(posn).ok_or(())? = element as u8;
                    posn += 1;
                    run -= 1;
                    continue;
                }
                let element = element - 256;
                let mut length = (element & 7) as i64;
                if length == 7 {
                    length += bits.symbol(&lengths)? as i64;
                }
                length += 2;
                let slot = element >> 3;
                let offset;
                match slot {
                    0 => offset = r0,
                    1 => {
                        offset = r1;
                        r1 = r0;
                        r0 = offset;
                    }
                    2 => {
                        offset = r2;
                        r2 = r0;
                        r0 = offset;
                    }
                    3 => {
                        offset = 1;
                        r2 = r1;
                        r1 = r0;
                        r0 = 1;
                    }
                    _ => {
                        let extra = *extra.get(slot).ok_or(())?;
                        let mut o = base[slot] - 2;
                        if block_type == 2 && extra >= 3 {
                            o += ((bits.read(extra - 3) << 3) as i64) + bits.symbol(&aligned)? as i64;
                        } else if block_type == 2 && extra == 0 {
                            o = 1;
                        } else {
                            o += bits.read(extra) as i64;
                        }
                        offset = o;
                        r2 = r1;
                        r1 = r0;
                        r0 = offset;
                    }
                }
                let length = length as usize;
                if posn + length > size {
                    return Err(()); // "LZX match runs past the window"
                }
                let mut src = posn as i64 - offset;
                if src < 0 {
                    src += size as i64; // wraps into the reference data at the window's end
                }
                if src < 0 || src as usize >= size {
                    return Err(()); // the Python would index from the end or past it
                }
                // The Python's slice copies only run where source and target never overlap, so this byte loop (its
                // overlap path) gives the same bytes in every case.
                let src = src as usize;
                for k in 0..length {
                    window[posn + k] = window[(src + k) % size];
                }
                posn += length;
                run -= length as i64;
            }
            if run < 0 {
                block_remaining += run;
                if block_remaining < 0 {
                    return Err(()); // a match past the next block too: malformed
                }
            }
        }
        if posn as i64 - frame_posn as i64 != frame_size as i64 {
            return Err(()); // "LZX frame size mismatch"
        }
        if bits.n > 0 {
            bits.ensure(16);
        }
        let k = bits.n & 15;
        bits.read(k);
        if intel_size != 0 {
            return Err(()); // "E8 translation is not expected in an Xbox 360 image"
        }
        out[written..written + frame_size].copy_from_slice(&window[frame_posn..frame_posn + frame_size]);
        written += frame_size;
        frame_posn += frame_size;
        posn %= size;
        frame_posn %= size;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn skate_lzx(data: *const u8, len: usize, out_len: usize, window_bits: u32, reference: *const u8,
                                   ref_len: usize, out: *mut u8) -> i32 {
    if data.is_null() || out.is_null() || (reference.is_null() && ref_len != 0) {
        return 1;
    }
    let data = unsafe { std::slice::from_raw_parts(data, len) };
    let reference = if ref_len == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(reference, ref_len) } };
    let out = unsafe { std::slice::from_raw_parts_mut(out, out_len) };
    if lzx(data, out_len, window_bits, reference, out).is_ok() { 0 } else { 1 }
}
