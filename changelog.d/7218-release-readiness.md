Restores release readiness after recent API and generated-evidence drift. The
runtime now closes the remaining iterator, JSON, stream, fetch, zlib, class
capture, constructor-arity, and diagnostics parity regressions; release
fixtures and known-failure inventories match current behavior again; and lint,
warning, documentation, audit, and public benchmark gates are reproducible.

Also advances the workspace to `0.5.1279`, correcting the accidental version
regression from `0.5.1278` to `0.5.1277` in #7196.
