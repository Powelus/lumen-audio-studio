# Notes

- Audio-thread code must not allocate, lock or block.
- Floating point: use `f32` for audio buffers and `f64` for coefficient maths,
  like the web version.
