//! Everything in the own backend that calls Windows: WGC capture, the D3D11
//! texture path, Media Foundation encoding, and WASAPI audio.
//!
//! **Empty — #236 (WS1.6.4) fills it**, starting with WGC video through the
//! sink writer. Only this module is gated to Windows; the decisions it will
//! act on live beside it in `clock`, `pcm` and `select`, which compile and are
//! tested everywhere.
