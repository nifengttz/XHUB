# Fuzz targets

These targets cover the fixed-width protocol decoders and arbitrary CLVM tree
encodings. They are separate from the normal test binary so libFuzzer can run
them with process isolation.

```powershell
cargo fuzz run protocol_bytes -- -max_total_time=300
cargo fuzz run clvm_solution -- -max_total_time=300
```

A crash artifact is a release blocker until it has a regression test and a
documented disposition.
