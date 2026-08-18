# `bench/perf/` — 12 programs × {C, Rust, Go, Ax} ([T-7.1.1])

Programs: binary-trees, n-body, spectral-norm, fannkuch-redux, mandelbrot,
JSON parse 100 MB, word frequency, ray tracer, regex scan, matmul,
B-tree insert/lookup, LZ4-style compress.

Each language implementation must be algorithmically equivalent. Comparing
different algorithms is self-deception.

Existing kernels live in `ax bench metrics` / `ax bench gate`. This directory
is the T-spec home for reviewed four-language sources as they land.
