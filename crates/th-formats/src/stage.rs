//! STD stage background files (TH10+ layout).
//!
//! A stage is a set of *objects* (each a list of textured quads whose sprites come
//! from ANM scripts), a list of *instances* placing those objects in 3D space, and a
//! timed *script* that drives the camera, fog and similar effects.
//!
//! Layout from thtk's thstd.c (BSD-2-Clause).

use crate::{Error, Reader, Result};

#[derive(Clone, Debug)]
pub struct Quad {
    /// Quad type (0 = plain rectangle in older games; other values exist).
    pub kind: u16,
    /// ANM script index (in the stage's ANM file) that animates this quad.
    pub script: u16,
    pub pos: [f32; 3],
    pub size: [f32; 2],
}

#[derive(Clone, Debug)]
pub struct Object {
    pub id: u16,
    pub flags: u16,
    pub pos: [f32; 3],
    pub size: [f32; 3],
    pub quads: Vec<Quad>,
}

#[derive(Clone, Debug)]
pub struct Instance {
    pub object: u16,
    pub pos: [f32; 3],
}

#[derive(Clone, Debug)]
pub struct Instr {
    pub time: i32,
    pub opcode: u16,
    /// Byte offset from the start of the script (jump targets use these).
    pub offset: u32,
    pub args: Vec<u8>,
}

impl Instr {
    pub fn arg_i32(&self, n: usize) -> i32 {
        self.args
            .get(n * 4..n * 4 + 4)
            .map_or(0, |b| i32::from_le_bytes(b.try_into().unwrap()))
    }
    pub fn arg_f32(&self, n: usize) -> f32 {
        f32::from_bits(self.arg_i32(n) as u32)
    }
}

#[derive(Clone, Debug, Default)]
pub struct StdFile {
    pub anm_name: String,
    pub objects: Vec<Object>,
    pub instances: Vec<Instance>,
    pub script: Vec<Instr>,
}

const HEADER_SIZE: usize = 144;
const OBJECT_HEADER: usize = 28;
const QUAD_SIZE: usize = 28;
const QUAD_END: u32 = 0x0004_FFFF;

fn vec3(r: &Reader, off: usize) -> Result<[f32; 3]> {
    Ok([r.peek_f32(off)?, r.peek_f32(off + 4)?, r.peek_f32(off + 8)?])
}

impl StdFile {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let h = Reader::at(data, 0)?;
        let n_objects = h.peek_u16(0)? as usize;
        let faces_offset = h.peek_u32(4)? as usize;
        let script_offset = h.peek_u32(8)? as usize;
        let anm_name = h.peek_cstr(16)?;

        let mut objects = Vec::with_capacity(n_objects);
        for i in 0..n_objects {
            let off = h.peek_u32(HEADER_SIZE + i * 4)? as usize;
            let o = Reader::at(data, off)?;
            let mut quads = Vec::new();
            let mut p = OBJECT_HEADER;
            loop {
                if o.peek_u32(p)? == QUAD_END {
                    break;
                }
                let size = (o.peek_u16(p + 2)? as usize).max(QUAD_SIZE);
                quads.push(Quad {
                    kind: o.peek_u16(p)?,
                    script: o.peek_u16(p + 4)?,
                    pos: vec3(&o, p + 8)?,
                    size: [o.peek_f32(p + 20)?, o.peek_f32(p + 24)?],
                });
                p += size;
                if quads.len() > 100_000 {
                    return Err(Error::Format("runaway quad list".into()));
                }
            }
            objects.push(Object {
                id: o.peek_u16(0)?,
                flags: o.peek_u16(2)?,
                pos: vec3(&o, 4)?,
                size: vec3(&o, 16)?,
                quads,
            });
        }

        let mut instances = Vec::new();
        let mut p = faces_offset;
        loop {
            let id = h.peek_u16(p)?;
            if id == 0xFFFF {
                break;
            }
            instances.push(Instance { object: id, pos: vec3(&h, p + 4)? });
            p += 16;
        }

        let mut script = Vec::new();
        let mut p = script_offset;
        loop {
            let size = h.peek_u16(p + 6)? as usize;
            if size == 0xFFFF {
                break;
            }
            if size < 8 {
                return Err(Error::Format(format!("bad STD instruction size {size} at {p:#x}")));
            }
            script.push(Instr {
                time: h.peek_i32(p)?,
                opcode: h.peek_u16(p + 4)?,
                offset: (p - script_offset) as u32,
                args: h.peek_bytes(p + 8, size - 8)?.to_vec(),
            });
            p += size;
        }

        Ok(Self { anm_name, objects, instances, script })
    }
}
