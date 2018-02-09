# Lazy Pool

A lazy-initialized object pool. Provides a sharable pool where objects
are initialized on demand. The pool works by providing Futures which allow
for usage with async/await (untested) and threading as well.

See tests for examples of usage
