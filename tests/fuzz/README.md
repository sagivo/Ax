# Fuzz corpora ([T-9])

Generators live in `ax::testharness` (`generate_aliasing`, `generate_semantics`).
Reduction is `testharness::reduce`. EMI is `testharness::emi_preserves`.

Gates are CPU-hours, not input counts ([T-9.1.2]). The cargo testharness
runs a short smoke of each generator so a broken generator fails CI.
