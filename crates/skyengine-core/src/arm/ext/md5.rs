use super::*;

impl ExtRuntime {
    pub(super) fn md5_init(&mut self, context: GuestAddr) -> Result<()> {
        self.memory.write_u32(context, 0)?;
        self.memory.write_u32(context.checked_add(4)?, 0)?;
        for (index, value) in [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476]
            .into_iter()
            .enumerate()
        {
            self.memory
                .write_u32(context.checked_add(8 + index as u32 * 4)?, value)?;
        }
        self.memory
            .write(context.checked_add(MD5_BUFFER_OFFSET)?, &[0; 64])
    }

    pub(super) fn md5_append(&mut self, context: GuestAddr, input: &[u8]) -> Result<()> {
        let total = u64::from(self.memory.read_u32(context)?)
            | (u64::from(self.memory.read_u32(context.checked_add(4)?)?) << 32);
        let next_total = total
            .checked_add(input.len() as u64)
            .ok_or_else(|| Error::Abi("MD5 byte count overflow".into()))?;
        let mut state = [0_u32; 4];
        for (index, value) in state.iter_mut().enumerate() {
            *value = self
                .memory
                .read_u32(context.checked_add(8 + index as u32 * 4)?)?;
        }
        let mut buffer: [u8; 64] = self
            .memory
            .read(context.checked_add(MD5_BUFFER_OFFSET)?, 64)?
            .try_into()
            .expect("checked MD5 buffer length");
        md5_consume(&mut state, &mut buffer, (total % 64) as usize, input);

        self.memory.write_u32(context, next_total as u32)?;
        self.memory
            .write_u32(context.checked_add(4)?, (next_total >> 32) as u32)?;
        for (index, value) in state.into_iter().enumerate() {
            self.memory
                .write_u32(context.checked_add(8 + index as u32 * 4)?, value)?;
        }
        self.memory
            .write(context.checked_add(MD5_BUFFER_OFFSET)?, &buffer)
    }

    pub(super) fn md5_finish(&mut self, context: GuestAddr, output: GuestAddr) -> Result<()> {
        let total = u64::from(self.memory.read_u32(context)?)
            | (u64::from(self.memory.read_u32(context.checked_add(4)?)?) << 32);
        let mut state = [0_u32; 4];
        for (index, value) in state.iter_mut().enumerate() {
            *value = self
                .memory
                .read_u32(context.checked_add(8 + index as u32 * 4)?)?;
        }
        let mut buffer: [u8; 64] = self
            .memory
            .read(context.checked_add(MD5_BUFFER_OFFSET)?, 64)?
            .try_into()
            .expect("checked MD5 buffer length");
        let buffered = (total % 64) as usize;
        let padding_len = if buffered < 56 {
            56 - buffered
        } else {
            120 - buffered
        };
        let mut padding = vec![0; padding_len + 8];
        padding[0] = 0x80;
        padding[padding_len..].copy_from_slice(&total.wrapping_mul(8).to_le_bytes());
        let remaining = md5_consume(&mut state, &mut buffer, buffered, &padding);
        debug_assert_eq!(remaining, 0);

        let mut digest = [0_u8; 16];
        for (chunk, value) in digest.as_chunks_mut::<4>().0.iter_mut().zip(state) {
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        self.memory.write(output, &digest)
    }
}

fn md5_consume(
    state: &mut [u32; 4],
    buffer: &mut [u8; 64],
    mut buffered: usize,
    mut input: &[u8],
) -> usize {
    if buffered != 0 {
        let copied = (64 - buffered).min(input.len());
        buffer[buffered..buffered + copied].copy_from_slice(&input[..copied]);
        buffered += copied;
        input = &input[copied..];
        if buffered == 64 {
            md5_transform(state, buffer);
            buffered = 0;
        }
    }
    while input.len() >= 64 {
        let block: &[u8; 64] = input[..64].try_into().expect("checked MD5 block length");
        md5_transform(state, block);
        input = &input[64..];
    }
    if !input.is_empty() {
        buffer[..input.len()].copy_from_slice(input);
        buffered = input.len();
    }
    buffered
}

fn md5_transform(state: &mut [u32; 4], block: &[u8; 64]) {
    const SHIFTS: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const CONSTANTS: [u32; 64] = [
        0xd76a_a478,
        0xe8c7_b756,
        0x2420_70db,
        0xc1bd_ceee,
        0xf57c_0faf,
        0x4787_c62a,
        0xa830_4613,
        0xfd46_9501,
        0x6980_98d8,
        0x8b44_f7af,
        0xffff_5bb1,
        0x895c_d7be,
        0x6b90_1122,
        0xfd98_7193,
        0xa679_438e,
        0x49b4_0821,
        0xf61e_2562,
        0xc040_b340,
        0x265e_5a51,
        0xe9b6_c7aa,
        0xd62f_105d,
        0x0244_1453,
        0xd8a1_e681,
        0xe7d3_fbc8,
        0x21e1_cde6,
        0xc337_07d6,
        0xf4d5_0d87,
        0x455a_14ed,
        0xa9e3_e905,
        0xfcef_a3f8,
        0x676f_02d9,
        0x8d2a_4c8a,
        0xfffa_3942,
        0x8771_f681,
        0x6d9d_6122,
        0xfde5_380c,
        0xa4be_ea44,
        0x4bde_cfa9,
        0xf6bb_4b60,
        0xbebf_bc70,
        0x289b_7ec6,
        0xeaa1_27fa,
        0xd4ef_3085,
        0x0488_1d05,
        0xd9d4_d039,
        0xe6db_99e5,
        0x1fa2_7cf8,
        0xc4ac_5665,
        0xf429_2244,
        0x432a_ff97,
        0xab94_23a7,
        0xfc93_a039,
        0x655b_59c3,
        0x8f0c_cc92,
        0xffef_f47d,
        0x8584_5dd1,
        0x6fa8_7e4f,
        0xfe2c_e6e0,
        0xa301_4314,
        0x4e08_11a1,
        0xf753_7e82,
        0xbd3a_f235,
        0x2ad7_d2bb,
        0xeb86_d391,
    ];

    let mut words = [0_u32; 16];
    for (word, bytes) in words.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_le_bytes(*bytes);
    }
    let [mut a, mut b, mut c, mut d] = *state;
    for index in 0..64 {
        let (function, word) = match index {
            0..=15 => ((b & c) | (!b & d), index),
            16..=31 => ((d & b) | (!d & c), (5 * index + 1) % 16),
            32..=47 => (b ^ c ^ d, (3 * index + 5) % 16),
            _ => (c ^ (b | !d), (7 * index) % 16),
        };
        let next = b.wrapping_add(
            a.wrapping_add(function)
                .wrapping_add(CONSTANTS[index])
                .wrapping_add(words[word])
                .rotate_left(SHIFTS[index]),
        );
        a = d;
        d = c;
        c = b;
        b = next;
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}
