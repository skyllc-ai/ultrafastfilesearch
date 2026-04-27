// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Memory-tiered storage backing for compact-index columns.
//!
//! Phase 2 of the memory-tiering rollout (see
//! `docs/refactor/memory-tiering-implementation-plan.md`).  Wraps the
//! two largest [`crate::compact::DriveCompactIndex`] columns
//! ([`crate::compact::CompactRecord`] and `names: Vec<u8>`) so the
//! same field type can carry either a heap-resident `Vec<T>` (Phase 2a
//! — the only path before this rollout) or a read-only typed view onto
//! a memory-mapped runtime tempfile (Phase 2b).  Read-side call sites
//! see no difference; mutation transparently promotes mmap-backed
//! columns to the heap.
//!
//! ## API surface
//!
//! Read-side:  [`Deref<Target = [T]>`] is implemented, so almost every
//! existing call site (`column.len()`, `column.iter()`, `&column[i]`,
//! `for x in &column`) works unchanged.
//!
//! Mutation-side:  callers that need `Vec`-specific methods (`push`,
//! `extend_from_slice`, `shrink_to_fit`, `reserve`) call
//! [`ColumnStorage::as_mut_vec`].  For [`ColumnStorage::Vec`] this is
//! a cheap reference; for [`ColumnStorage::Mmap`] it triggers a
//! one-time copy into a fresh `Vec<T>` and replaces the variant.  All
//! mutating accessors funnel through the private
//! `materialise_if_mmap` helper, so the variant transition is
//! single-site.
//!
//! ## Variants
//!
//! - [`ColumnStorage::Vec`] — heap-allocated, owned, mutable.  Reads are
//!   zero-cost; mutation is direct.
//! - [`ColumnStorage::Mmap`] — read-only view onto a memory-mapped region.
//!   Reads slice the mmap directly via [`bytemuck::cast_slice`] with zero
//!   allocation.  Mutating operations (`as_mut_slice`, `as_mut_vec`,
//!   `IndexMut`, `DerefMut`) transparently promote to a heap-resident
//!   [`Vec<T>`] on first call — the column "materialises" into the heap, the
//!   mmap stays alive (referenced by other [`Arc<Mmap>`] holders) and is
//!   dropped when the last reference goes away.
//!
//! ## Mmap-variant invariants
//!
//! Construction goes through [`ColumnStorage::from_mmap_region`] which
//! validates:
//!
//! 1. `byte_offset + len * size_of::<T>()` does not exceed the mmap.
//! 2. The base address (`mmap.as_ptr() as usize + byte_offset`) is aligned to
//!    `align_of::<T>()`.
//!
//! Both checks are required for [`bytemuck::cast_slice`] to be sound on
//! the slice we hand out from [`ColumnStorage::as_slice`].  Production callers
//! obtain aligned regions from `crate::compact_mmap::RuntimeLayout` which
//! page-aligns every column header; the validation here is
//! belt-and-braces against future callers.

use alloc::sync::Arc;
use core::marker::PhantomData;
use core::mem::{align_of, size_of};
use core::ops::{Deref, DerefMut, Index, IndexMut};
use core::slice::SliceIndex;

use memmap2::Mmap;

/// Reasons a [`ColumnStorage::from_mmap_region`] call can reject a
/// candidate `(mmap, byte_offset, len)` triple.
///
/// All variants are caller errors — a correctly-built
/// `crate::compact_mmap::RuntimeLayout` never produces them.  We surface
/// them as a typed enum (rather than `&'static str` like the legacy
/// `compact_cache::deserialize_compact` errors) so future callers can
/// match on them in `Result` chains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmapRegionError {
    /// `byte_offset + len * size_of::<T>()` overflows `usize`.
    Overflow,
    /// `byte_offset + len * size_of::<T>()` exceeds `mmap.len()`.
    OutOfBounds {
        /// First byte of the requested slice (inclusive).
        start: usize,
        /// One-past-the-last byte of the requested slice.
        end: usize,
        /// Length of the backing mmap in bytes.
        mmap_len: usize,
    },
    /// `(mmap.as_ptr() as usize + byte_offset) % align_of::<T>() != 0`.
    Misaligned {
        /// `align_of::<T>()` — what the slice would need.
        required: usize,
        /// `(mmap.as_ptr() as usize + byte_offset) % required` — how
        /// far off we are.  Always in `1..required`.
        actual_offset: usize,
    },
}

impl core::fmt::Display for MmapRegionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Overflow => f.write_str(
                "mmap region length overflows usize (byte_offset + len * size_of::<T>())",
            ),
            Self::OutOfBounds {
                start,
                end,
                mmap_len,
            } => write!(
                f,
                "mmap region {start}..{end} exceeds backing mmap of {mmap_len} bytes",
            ),
            Self::Misaligned {
                required,
                actual_offset,
            } => write!(
                f,
                "mmap region misaligned: needs {required}-byte alignment, off by {actual_offset} bytes",
            ),
        }
    }
}

impl core::error::Error for MmapRegionError {}

/// Memory-tiered storage backing for a single compact-index column.
///
/// Always parameterised by `T: bytemuck::Pod` so Phase 2b's mmap
/// variant can reinterpret the underlying byte slice as `&[T]`
/// without any per-element conversion.
///
/// # Variants
///
/// - [`ColumnStorage::Vec`] — heap-allocated, owned, mutable.  Reads are
///   zero-cost; mutation is direct.
/// - [`ColumnStorage::Mmap`] — read-only mmap region with a typed view.
///   Lifetime is tied to a `crate::compact_mmap::RuntimeLayout` (a per-process
///   tempfile `mmap`'d read-only).  Mutating an `Mmap`-backed column triggers a
///   one-time copy into a fresh `Vec` (the column "promotes back" to heap), at
///   which point further mutations are free.  The original `Arc<Mmap>` is
///   dropped at the point of promotion; if other [`ColumnStorage`] columns
///   still reference it (because they share the same runtime tempfile), the
///   mmap stays alive on their behalf.
///
/// # Why not `Cow<[T]>`
///
/// `std::borrow::Cow` would borrow `&'a [T]` from somewhere with a
/// lifetime, forcing every owner of a `DriveCompactIndex` to also
/// own (or borrow) the mmap.  `ColumnStorage` instead carries the
/// `Arc<Mmap>` with the column itself, so `DriveCompactIndex` stays
/// `'static`.
pub enum ColumnStorage<T: bytemuck::Pod> {
    /// Heap-resident, mutable, owned bytes.
    Vec(Vec<T>),
    /// Read-only typed view onto a region of an mmap'd file.
    ///
    /// Constructed via [`ColumnStorage::from_mmap_region`].  Mutating
    /// operations — `as_mut_slice`, `as_mut_vec`, [`IndexMut`],
    /// [`DerefMut`] — transparently allocate a fresh [`Vec<T>`] and
    /// `*self = Self::Vec(...)`, after which the column behaves as
    /// the `Vec` variant.
    Mmap {
        /// Reference-counted backing mmap.  Multiple [`ColumnStorage`]
        /// columns can share one [`Arc<Mmap>`] when their byte ranges
        /// don't overlap (e.g. the records + names + trigram columns of
        /// a single drive's runtime layout).
        mmap: Arc<Mmap>,
        /// Offset of the typed slice within `mmap.as_ref()`, in bytes.
        byte_offset: usize,
        /// Number of `T` elements in the slice.
        len: usize,
        /// Phantom marker so the variant is generic over `T` without
        /// holding a `T`.
        _phantom: PhantomData<T>,
    },
}

impl<T: bytemuck::Pod> ColumnStorage<T> {
    /// Wrap an existing [`Vec`].  Allocation-free.
    #[must_use]
    pub const fn from_vec(vec: Vec<T>) -> Self {
        Self::Vec(vec)
    }

    /// Build a read-only mmap-backed column over a region of `mmap`.
    ///
    /// `byte_offset` is the start of the typed slice within
    /// `mmap.as_ref()`; `len` is the number of `T` elements (not
    /// bytes).  The total span `[byte_offset, byte_offset + len *
    /// size_of::<T>())` must fit in the mmap, and the base address
    /// must be aligned to `align_of::<T>()`.
    ///
    /// # Errors
    ///
    /// - [`MmapRegionError::Overflow`] if `len * size_of::<T>()` or
    ///   `byte_offset + that` overflows `usize`.
    /// - [`MmapRegionError::OutOfBounds`] if the span exceeds `mmap.len()`.
    /// - [`MmapRegionError::Misaligned`] if the base address fails the
    ///   alignment check.
    ///
    /// Production callers (`crate::compact_mmap::load_from_runtime`)
    /// receive a layout that page-aligns every column header, which
    /// satisfies any `T: Pod` alignment up to the page size.
    pub fn from_mmap_region(
        mmap: Arc<Mmap>,
        byte_offset: usize,
        len: usize,
    ) -> Result<Self, MmapRegionError> {
        let elem_size = size_of::<T>();
        // ZSTs would make `len * 0 = 0` and confuse the bounds check;
        // also, no compact-index column type is a ZST.  Reject for
        // future-proofing.  bytemuck::Pod doesn't preclude ZSTs at
        // the trait level, only at runtime.
        debug_assert!(elem_size > 0, "ColumnStorage<T> with ZST T is meaningless");
        let bytes_needed = len
            .checked_mul(elem_size)
            .ok_or(MmapRegionError::Overflow)?;
        let end = byte_offset
            .checked_add(bytes_needed)
            .ok_or(MmapRegionError::Overflow)?;
        let mmap_len = mmap.len();
        if end > mmap_len {
            return Err(MmapRegionError::OutOfBounds {
                start: byte_offset,
                end,
                mmap_len,
            });
        }
        let alignment = align_of::<T>();
        // Read the pointer's address as `usize` purely for an alignment
        // check; we never deref it.  Casting `*const u8` through
        // `usize` is the standard idiom — `<*const T>::addr()` would
        // also work but went stable later than this crate's MSRV needs
        // it to.  `wrapping_add` matches the address-arithmetic
        // semantics expected here (we don't care about overflow because
        // the bounds check above already verified the slice fits).
        let base_addr = mmap.as_ptr() as usize;
        let actual_offset = (base_addr.wrapping_add(byte_offset)) % alignment;
        if actual_offset != 0 {
            return Err(MmapRegionError::Misaligned {
                required: alignment,
                actual_offset,
            });
        }
        Ok(Self::Mmap {
            mmap,
            byte_offset,
            len,
            _phantom: PhantomData,
        })
    }

    /// Borrow the column as a slice.  Always allocation-free.
    ///
    /// For the [`ColumnStorage::Mmap`] variant this slices the mmap
    /// directly via [`bytemuck::cast_slice`].  Soundness rests on the
    /// invariants validated by [`Self::from_mmap_region`].
    ///
    /// # Panics
    ///
    /// Cannot panic in practice: [`Self::from_mmap_region`] verifies
    /// at construction time that `byte_offset + len * size_of::<T>()`
    /// fits in `mmap.len()` and that the base address is
    /// `align_of::<T>()`-aligned.  Reaching the panic paths in either
    /// the slice indexing or [`bytemuck::cast_slice`] would require
    /// those invariants to be violated after construction — impossible
    /// without `unsafe`, which the crate forbids.
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        match self {
            Self::Vec(vec) => vec.as_slice(),
            Self::Mmap {
                mmap,
                byte_offset,
                len,
                _phantom,
            } => {
                let bytes_len = *len * size_of::<T>();
                #[expect(
                    clippy::indexing_slicing,
                    reason = "byte_offset + bytes_len <= mmap.len() validated by from_mmap_region"
                )]
                let bytes = &mmap.as_ref()[*byte_offset..*byte_offset + bytes_len];
                bytemuck::cast_slice::<u8, T>(bytes)
            }
        }
    }

    /// Borrow the column as a mutable slice.
    ///
    /// If the column is mmap-backed, this materialises into a fresh
    /// `Vec` first so the caller observes a writable slice.  The mmap
    /// is then dropped from this column (other `ColumnStorage`s
    /// holding the same `Arc<Mmap>` keep their references).  Once
    /// promoted, the column stays heap-resident for the lifetime of
    /// the [`crate::compact::DriveCompactIndex`].
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        self.materialise_to_vec().as_mut_slice()
    }

    /// Promote to a [`Vec`] if not already, and return a mutable
    /// reference.
    ///
    /// Used by callers that need `Vec`-specific methods that are not
    /// part of the slice API: [`Vec::push`], [`Vec::extend_from_slice`],
    /// [`Vec::shrink_to_fit`], [`Vec::reserve`].  The Windows USN-patch
    /// path in `crate::compact_loader::apply_usn_patch` is the
    /// canonical caller.
    ///
    /// Triggers a one-time `mmap → Vec` copy on the first call against
    /// an `Mmap`-backed column.  Subsequent calls are cheap.
    pub fn as_mut_vec(&mut self) -> &mut Vec<T> {
        self.materialise_to_vec()
    }

    /// Consume and return the inner [`Vec`].
    ///
    /// For the [`ColumnStorage::Mmap`] variant, copies from the mmap.
    /// The `Arc<Mmap>` reference is dropped after the copy.
    ///
    /// # Panics
    ///
    /// Cannot panic in practice; same invariant story as
    /// [`Self::as_slice`].
    #[must_use]
    pub fn into_vec(self) -> Vec<T> {
        match self {
            Self::Vec(vec) => vec,
            Self::Mmap {
                mmap,
                byte_offset,
                len,
                _phantom,
            } => {
                let bytes_len = len * size_of::<T>();
                #[expect(
                    clippy::indexing_slicing,
                    reason = "byte_offset + bytes_len <= mmap.len() validated by from_mmap_region"
                )]
                let bytes = &mmap.as_ref()[byte_offset..byte_offset + bytes_len];
                bytemuck::cast_slice::<u8, T>(bytes).to_vec()
            }
        }
    }

    /// Capacity of the underlying storage in elements.
    ///
    /// For [`ColumnStorage::Vec`], delegates to [`Vec::capacity`].
    /// For [`ColumnStorage::Mmap`], returns [`Self::len`] — mmap
    /// pages have no extra slack.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        match self {
            Self::Vec(vec) => vec.capacity(),
            Self::Mmap { len, .. } => *len,
        }
    }

    /// Number of elements in the column.
    #[must_use]
    pub const fn len(&self) -> usize {
        match self {
            Self::Vec(vec) => vec.len(),
            Self::Mmap { len, .. } => *len,
        }
    }

    /// `true` if the column has zero elements.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `true` if the column is backed by [`ColumnStorage::Mmap`].
    ///
    /// Phase 2b memory-tiering regression-test hook
    /// (`docs/refactor/memory-tiering-implementation-plan.md` §3
    /// Phase 2b Commit F).  Letting tests assert the storage variant
    /// directly avoids pattern-matching on the public enum, which
    /// would couple test code to the variant order and break on a
    /// future third variant.
    #[must_use]
    pub const fn is_mmap(&self) -> bool {
        matches!(self, Self::Mmap { .. })
    }

    /// `true` if the column is backed by [`ColumnStorage::Vec`].
    ///
    /// Complement of [`Self::is_mmap`]; today the two are exhaustive
    /// (`is_mmap() == !is_vec()`) but kept symmetric so adding a
    /// future variant does not silently flip the meaning of either
    /// accessor.
    #[must_use]
    pub const fn is_vec(&self) -> bool {
        matches!(self, Self::Vec(_))
    }

    // ─────────────────────────────────────────────────────────────────
    // Internal: promotion logic.
    // ─────────────────────────────────────────────────────────────────

    /// Promote `self` to the [`ColumnStorage::Vec`] variant if it
    /// isn't already, then return a `&mut Vec<T>`.
    ///
    /// Single source of truth for [`Self::as_mut_slice`] and
    /// [`Self::as_mut_vec`]: both delegate here.  For the `Vec`
    /// variant the call is `O(1)` (a re-borrow); for the `Mmap`
    /// variant it allocates a fresh `Vec<T>` and `*self = Self::Vec`,
    /// so future calls are also `O(1)`.
    fn materialise_to_vec(&mut self) -> &mut Vec<T> {
        if matches!(self, Self::Mmap { .. }) {
            // Borrow ends before the assignment because `to_vec()`
            // returns an owned `Vec<T>` independent of `self`.
            let copy: Vec<T> = self.as_slice().to_vec();
            *self = Self::Vec(copy);
        }
        match self {
            Self::Vec(vec) => vec,
            Self::Mmap { .. } => {
                // The `if matches!(...)` block above promoted any
                // `Mmap` variant to `Vec`.  We hold `&mut self`
                // exclusively so no concurrent mutation can change
                // the variant between the check and this match —
                // reaching this arm would require interior mutability
                // (impossible for an enum field with no `Cell`/`Mutex`)
                // or a logic error in the helper.  We surface the
                // logic-error case as `unreachable!()` rather than
                // returning a placeholder Vec so a future bug can't
                // silently corrupt the column.
                #[expect(
                    clippy::unreachable,
                    reason = "materialise step above promoted Mmap to Vec; \
                              we hold &mut self exclusively, so the variant \
                              cannot change between the check and the match"
                )]
                {
                    unreachable!("materialise_to_vec left non-Vec variant")
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Standard trait impls — minimise call-site churn.
// ─────────────────────────────────────────────────────────────────────

impl<T: bytemuck::Pod> Deref for ColumnStorage<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T: bytemuck::Pod> DerefMut for ColumnStorage<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T: bytemuck::Pod> Default for ColumnStorage<T> {
    /// Empty heap-backed column.
    fn default() -> Self {
        Self::Vec(Vec::new())
    }
}

impl<T: bytemuck::Pod> From<Vec<T>> for ColumnStorage<T> {
    fn from(vec: Vec<T>) -> Self {
        Self::from_vec(vec)
    }
}

impl<T: bytemuck::Pod> Clone for ColumnStorage<T> {
    /// Always clones into a heap-resident [`Vec`].  Clones never share
    /// an `Mmap` across [`ColumnStorage`] instances; that invariant
    /// keeps the promote-on-mutation logic single-owner per column and
    /// avoids `Arc<Mmap>` reference cycles between shards.  Clones of
    /// `Mmap`-variant columns therefore allocate; the operation is
    /// `O(len * size_of::<T>())`.
    fn clone(&self) -> Self {
        Self::Vec(self.as_slice().to_vec())
    }
}

impl<T: bytemuck::Pod + core::fmt::Debug> core::fmt::Debug for ColumnStorage<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let variant = match self {
            Self::Vec(_) => "Vec",
            Self::Mmap { .. } => "Mmap",
        };
        f.debug_struct("ColumnStorage")
            .field("variant", &variant)
            .field("data", &self.as_slice())
            .finish()
    }
}

impl<T: bytemuck::Pod, I: SliceIndex<[T]>> Index<I> for ColumnStorage<T> {
    type Output = I::Output;

    #[expect(
        clippy::indexing_slicing,
        reason = "Index trait's contract is to panic on out-of-bounds; \
                  forwarding to the slice is the documented behaviour."
    )]
    fn index(&self, index: I) -> &Self::Output {
        &self.as_slice()[index]
    }
}

impl<T: bytemuck::Pod, I: SliceIndex<[T]>> IndexMut<I> for ColumnStorage<T> {
    #[expect(
        clippy::indexing_slicing,
        reason = "IndexMut trait's contract is to panic on out-of-bounds; \
                  forwarding to the slice is the documented behaviour."
    )]
    fn index_mut(&mut self, index: I) -> &mut Self::Output {
        &mut self.as_mut_slice()[index]
    }
}

impl<'a, T: bytemuck::Pod> IntoIterator for &'a ColumnStorage<T> {
    type IntoIter = core::slice::Iter<'a, T>;
    type Item = &'a T;

    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}

impl<'a, T: bytemuck::Pod> IntoIterator for &'a mut ColumnStorage<T> {
    type IntoIter = core::slice::IterMut<'a, T>;
    type Item = &'a mut T;

    fn into_iter(self) -> Self::IntoIter {
        self.as_mut_slice().iter_mut()
    }
}

#[cfg(test)]
mod tests;
