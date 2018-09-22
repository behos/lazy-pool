# Lazy Pool

A lazy-initialized object pool. Provides a sharable pool where objects
are initialized on demand. The pool works by providing Futures which allow
for usage with async/await (untested) and threading as well.

See tests for examples of usage

# Release Notes

## 0.2.3

* Introduce mutable dereferencing

## 0.2.1

* Minor fixes and switch to VecDeque

## 0.2.0

* Deprecated all previous version due to misusing Future trait
* Allow definition of a factory using closures. This adds the overhead of needing a box for function references as well
