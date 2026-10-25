//! The MPEG-4 Part 2 headers a Simple-profile stream carries (14496-2, "Video Object Layer" and
//! "Video Object Plane"): the video object layer, which the intro movies repeat before each
//! I-VOP, and the VOP header.

use crate::bits::BitReader;
use crate::Unsupported;

/// What a VOL header sets that decoding reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Vol {
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// The bits of `vop_time_increment`: enough for `vop_time_increment_resolution - 1`, at
    /// least one.
    pub(crate) time_increment_bits: u32,
}

/// A header field the decoder does not take, or a broken header.
#[derive(Debug)]
pub(crate) enum HeaderError {
    Unsupported(Unsupported),
    Corrupt(&'static str),
}

fn marker(r: &mut BitReader) -> Result<(), HeaderError> {
    if r.bit() {
        Ok(())
    } else {
        Err(HeaderError::Corrupt("missing marker bit"))
    }
}

fn unsupported<T>(what: Unsupported) -> Result<T, HeaderError> {
    Err(HeaderError::Unsupported(what))
}

/// Parse `video_object_layer()` from after its start code. Every field a rectangular,
/// progressive, 8-bit, H.263-quantised stream without sprites, partitioning or scalability sets is
/// read; any other tool is refused.
pub(crate) fn parse_vol(r: &mut BitReader) -> Result<Vol, HeaderError> {
    let _random_accessible_vol = r.bit();
    let _video_object_type_indication = r.read(8);
    let mut verid = 1;
    if r.bit() {
        verid = r.read(4);
        let _priority = r.read(3);
    }
    if r.read(4) == 0xf {
        let _par = r.read(16);
    }
    if r.bit() {
        let chroma_format = r.read(2);
        if chroma_format != 1 {
            return unsupported(Unsupported::ChromaFormat);
        }
        let _low_delay = r.bit();
        if r.bit() {
            // first and latter halves of bit_rate, vbv_buffer_size and vbv_occupancy.
            let _ = r.read(15);
            marker(r)?;
            let _ = r.read(15);
            marker(r)?;
            let _ = r.read(15);
            marker(r)?;
            let _ = r.read(3);
            let _ = r.read(11);
            marker(r)?;
            let _ = r.read(15);
            marker(r)?;
        }
    }
    if r.read(2) != 0 {
        return unsupported(Unsupported::Shape);
    }
    marker(r)?;
    let resolution = r.read(16);
    if resolution == 0 {
        return Err(HeaderError::Corrupt("zero vop_time_increment_resolution"));
    }
    marker(r)?;
    let time_increment_bits = (32 - (resolution - 1).leading_zeros()).max(1);
    if r.bit() {
        let _fixed_vop_time_increment = r.read(time_increment_bits);
    }
    marker(r)?;
    let width = r.read(13);
    marker(r)?;
    let height = r.read(13);
    marker(r)?;
    if width == 0 || height == 0 {
        return Err(HeaderError::Corrupt("zero VOL size"));
    }
    if r.bit() {
        return unsupported(Unsupported::Interlaced);
    }
    if !r.bit() {
        return unsupported(Unsupported::Obmc);
    }
    if r.read(if verid == 1 { 1 } else { 2 }) != 0 {
        return unsupported(Unsupported::Sprite);
    }
    if r.bit() {
        return unsupported(Unsupported::Not8Bit);
    }
    if r.bit() {
        return unsupported(Unsupported::Mpeg2Quant);
    }
    if verid != 1 && r.bit() {
        return unsupported(Unsupported::QuarterPel);
    }
    if !r.bit() {
        return unsupported(Unsupported::ComplexityEstimation);
    }
    if !r.bit() {
        return unsupported(Unsupported::ResyncMarkers);
    }
    if r.bit() {
        return unsupported(Unsupported::DataPartitioning);
    }
    if verid != 1 {
        if r.bit() {
            return unsupported(Unsupported::NewPred);
        }
        if r.bit() {
            return unsupported(Unsupported::ReducedResolution);
        }
    }
    if r.bit() {
        return unsupported(Unsupported::Scalability);
    }
    if r.overrun() {
        return Err(HeaderError::Corrupt("truncated VOL header"));
    }
    Ok(Vol {
        width,
        height,
        time_increment_bits,
    })
}

/// The coding type of a VOP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VopType {
    I,
    P,
}

/// A coded VOP's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Vop {
    pub(crate) kind: VopType,
    pub(crate) rounding: bool,
    pub(crate) intra_dc_vlc_thr: u32,
    pub(crate) quant: u32,
    /// `vop_fcode_forward`, 1 for an I-VOP.
    pub(crate) fcode: u32,
}

/// Parse `video_object_plane()`'s header from after its start code: `None` for a VOP with
/// `vop_coded` 0, which repeats the previous picture.
pub(crate) fn parse_vop(r: &mut BitReader, vol: &Vol) -> Result<Option<Vop>, HeaderError> {
    let kind = match r.read(2) {
        0 => VopType::I,
        1 => VopType::P,
        2 => return unsupported(Unsupported::BVop),
        _ => return unsupported(Unsupported::Sprite),
    };
    // modulo_time_base: a run of ones ended by a zero.
    let mut ones = 0;
    while r.bit() {
        ones += 1;
        if ones > 64 || r.overrun() {
            return Err(HeaderError::Corrupt("unterminated modulo_time_base"));
        }
    }
    marker(r)?;
    let _vop_time_increment = r.read(vol.time_increment_bits);
    marker(r)?;
    if !r.bit() {
        return Ok(None);
    }
    let rounding = kind == VopType::P && r.bit();
    let intra_dc_vlc_thr = r.read(3);
    let quant = r.read(5);
    if quant == 0 {
        return Err(HeaderError::Corrupt("zero vop_quant"));
    }
    let fcode = if kind == VopType::P { r.read(3) } else { 1 };
    if fcode == 0 {
        return Err(HeaderError::Corrupt("zero vop_fcode_forward"));
    }
    if r.overrun() {
        return Err(HeaderError::Corrupt("truncated VOP header"));
    }
    Ok(Some(Vop {
        kind,
        rounding,
        intra_dc_vlc_thr,
        quant,
        fcode,
    }))
}
