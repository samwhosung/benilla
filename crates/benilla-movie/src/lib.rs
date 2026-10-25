//! The 1.12.1 intro movies (`Interface/Cinematics/*.avi`): the AVI reader and a decoder for their
//! video, MPEG-4 Part 2 Simple profile (DivX 5, FourCC `DXGM`), written from ISO/IEC 14496-2.

mod bits;
mod idct;
mod scan;
mod vlc;
