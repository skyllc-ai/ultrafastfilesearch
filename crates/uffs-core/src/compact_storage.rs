// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Memory-tiered storage backing for compact-index columns.
//!
//! Phase 2a of the memory-tiering rollout (see
//! `docs/refactor/memory-tiering-implementation-plan.md`).  Wraps the
//! two largest [`crate::compact::DriveCompactIndex`] columns
//! ([`crate::compact::CompactRecord`] and `names: Vec<u8>`) so that
//! Phase 2b can later swap their backing from heap-resident `Vec<T>`
//! to a memory-mapped runtime tempfile without changing any read
//! site.
//!
//! ## API surface
//!
//! Read-side:  [`Deref<Target = [T]>`] is implemented, so almost every
//! existing call site (`column.len()`, `column.iter()`, `&column[i]`,
//! `for x in &column`) works unchanged.
//!
//! Mutation-side:  callers that need `Vec`-specific methods (`push`,
//! `extend_from_slice`, `shrink_to_fit`, `reserve`) call
//! [`ColumnStorage::as_mut_vec`].  In Phase 2a this is a no-op
//! reference; in Phase 2b it will trigger a one-time `Mmap → Vec`
//! materialisation when the underlying storage is mmap-backed.
//!
//! ## Phase 2b preview
//!
//! Phase 2b will add a second variant:
//!
//! ```ignore
//! Mmap {
//!     mmap: Arc<memmap2::Mmap>,
//!     byte_offset: usize,
//!     len: usize,
//!     _phantom: PhantomData<T>,
//! }
//! ```
//!
//! and the `as_mut_vec` / `into_vec` paths will allocate a fresh
//! `Vec` and copy from the mmap on first mutation.  Read paths
//! continue to slice the mmap directly with zero allocation.

use core::ops::{Deref, DerefMut, Index, IndexMut};
use core::slice::SliceIndex;

/// Memory-tiered storage backing for a single compact-index column.
///
/// Always parameterised by `T: bytemuck::Pod` so Phase 2b's mmap
/// variant can reinterpret the underlying byte slice as `&[T]`
/// without any per-element conversion.
///
/// # Variants
///
/// - [`ColumnStorage::Vec`] — heap-allocated, owned, mutable.  The only variant
///   Phase 2a constructs.  Reads are zero-cost.  Mutation is direct.
///
/// Phase 2b adds an `Mmap` variant whose lifetime is tied to a
/// `crate::compact_mmap::RuntimeLayout` (a per-process tempfile
/// `mmap`'d read-only).  Mutating an `Mmap`-backed column triggers a
/// one-time copy into a fresh `Vec` (the column "promotes back"
/// to heap), at which point further mutations are free.
///
/// # Why not `Cow<[T]>`
///
/// `std::borrow::Cow` would borrow `&'a [T]` from somewhere with a
/// lifetime, forcing every owner of a `DriveCompactIndex` to also
/// own (or borrow) the mmap.  `ColumnStorage` instead carries the
/// `Arc<Mmap>` (in the Phase 2b variant) with the column itself, so
/// `DriveCompactIndex` stays `'static`.
pub enum ColumnStorage<T: bytemuck::Pod> {
    /// Heap-resident, mutable, owned bytes.
    Vec(Vec<T>),
}

impl<T: bytemuck::Pod> ColumnStorage<T> {
    /// Wrap an existing [`Vec`].  Allocation-free.
    #[must_use]
    pub const fn from_vec(vec: Vec<T>) -> Self {
        Self::Vec(vec)
    }

    /// Borrow the column as a slice.  Always allocation-free.
    #[must_use]
    pub const fn as_slice(&self) -> &[T] {
        match self {
            Self::Vec(vec) => vec.as_slice(),
        }
    }

    /// Borrow the column as a mutable slice.
    ///
    /// Phase 2a: returns the inner `Vec`'s slice directly.
    ///
    /// Phase 2b: if the column is mmap-backed, materialises into a
    /// fresh `Vec` first so the caller observes a writable slice.
    /// Once promoted, the column stays heap-resident for the
    /// lifetime of the [`crate::compact::DriveCompactIndex`].
    pub const fn as_mut_slice(&mut self) -> &mut [T] {
        match self {
            Self::Vec(vec) => vec.as_mut_slice(),
        }
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
    /// Phase 2a: cheap reference (already `Vec`).
    ///
    /// Phase 2b: triggers a one-time `mmap → Vec` copy on the first
    /// call against an `Mmap`-backed column.  Subsequent calls are
    /// cheap.  The runtime tempfile is dropped on the next cache
    /// rebuild, not on this call.
    pub const fn as_mut_vec(&mut self) -> &mut Vec<T> {
        match self {
            Self::Vec(vec) => vec,
        }
    }

    /// Consume and return the inner [`Vec`].
    ///
    /// Phase 2b: copies from the mmap if the storage is `Mmap`-backed.
    #[must_use]
    pub fn into_vec(self) -> Vec<T> {
        match self {
            Self::Vec(vec) => vec,
        }
    }

    // NB: `into_vec` cannot be `const fn` because destructuring `self`
    // by value out of an enum variant is not yet const-stable.  The
    // other accessors are const because they only borrow.

    /// Capacity of the underlying storage in elements.
    ///
    /// For [`ColumnStorage::Vec`], delegates to [`Vec::capacity`].
    /// For Phase 2b's `Mmap` variant, returns [`Self::len`] (mmap
    /// pages have no extra slack).
    #[must_use]
    pub const fn capacity(&self) -> usize {
        match self {
            Self::Vec(vec) => vec.capacity(),
        }
    }

    /// Number of elements in the column.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.as_slice().len()
    }

    /// `true` if the column has zero elements.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
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
    /// Always clones into a heap-resident [`Vec`].  Clones never
    /// share an `Mmap` across [`ColumnStorage`] instances; that
    /// invariant keeps Phase 2b's promote-on-mutation logic
    /// single-owner and avoids `Arc<Mmap>` reference cycles between
    /// shards.
    fn clone(&self) -> Self {
        Self::Vec(self.as_slice().to_vec())
    }
}

impl<T: bytemuck::Pod + core::fmt::Debug> core::fmt::Debug for ColumnStorage<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("ColumnStorage")
            .field(&self.as_slice())
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
mod tests {
    use super::*;

    #[test]
    fn empty_default_has_zero_len_and_is_empty() {
        let column: ColumnStorage<u32> = ColumnStorage::default();
        assert_eq!(column.len(), 0);
        assert!(column.is_empty());
        assert_eq!(column.as_slice(), &[] as &[u32]);
    }

    #[test]
    fn from_vec_round_trips_through_into_vec() {
        let original = vec![1_u32, 2, 3, 4, 5];
        let column = ColumnStorage::from_vec(original.clone());
        assert_eq!(column.len(), 5);
        assert_eq!(column.as_slice(), original.as_slice());
        let recovered = column.into_vec();
        assert_eq!(recovered, original);
    }

    #[test]
    fn deref_lets_call_sites_use_slice_methods() {
        let column = ColumnStorage::from(vec![10_u32, 20, 30]);
        // Read-side operations dispatched via Deref.
        assert_eq!(column.first(), Some(&10));
        assert_eq!(column.last(), Some(&30));
        assert_eq!(column.iter().sum::<u32>(), 60);
        assert_eq!(column.get(1), Some(&20));
        let collected: Vec<u32> = (&column).into_iter().copied().collect();
        assert_eq!(collected, vec![10, 20, 30]);
    }

    #[test]
    fn deref_mut_lets_call_sites_mutate_in_place() {
        let mut column = ColumnStorage::from(vec![1_u32, 2, 3]);
        // Slice-style mutation via DerefMut.
        if let Some(slot) = column.get_mut(1) {
            *slot = 99;
        }
        assert_eq!(column.as_slice(), &[1_u32, 99, 3]);
    }

    #[test]
    fn as_mut_vec_supports_vec_specific_methods() {
        let mut column: ColumnStorage<u8> = ColumnStorage::default();
        column.as_mut_vec().push(7);
        column.as_mut_vec().extend_from_slice(&[8, 9, 10]);
        assert_eq!(column.as_slice(), &[7, 8, 9, 10]);
        column.as_mut_vec().shrink_to_fit();
        // shrink_to_fit may match capacity to len; either way, len stays.
        assert_eq!(column.len(), 4);
    }

    #[test]
    fn capacity_tracks_underlying_vec() {
        let mut buf: Vec<u32> = Vec::with_capacity(16);
        buf.push(1);
        let column = ColumnStorage::from_vec(buf);
        assert!(
            column.capacity() >= 16,
            "capacity must reflect underlying Vec"
        );
        assert_eq!(column.len(), 1);
    }

    #[test]
    fn clone_always_produces_a_vec_variant() {
        let original = ColumnStorage::from(vec![1_u32, 2, 3]);
        let mut copy = original.clone();
        assert_eq!(copy.as_slice(), original.as_slice());
        // Mutate the clone — original must remain unchanged.
        copy.as_mut_vec().push(4);
        assert_eq!(original.as_slice(), &[1_u32, 2, 3]);
        assert_eq!(copy.as_slice(), &[1_u32, 2, 3, 4]);
    }

    #[test]
    fn debug_format_shows_inner_slice() {
        let column = ColumnStorage::from(vec![1_u8, 2, 3]);
        let printed = format!("{column:?}");
        assert!(printed.contains("ColumnStorage"), "got: {printed}");
        assert!(printed.contains('1'), "got: {printed}");
    }
}
