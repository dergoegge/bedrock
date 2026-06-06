// SPDX-License-Identifier: GPL-2.0

//! Content-addressed shared page store for freeze-time COW deduplication.
//!
//! When a forked VM gains its first child (or is checkpointed via the
//! `DEDUP_VM` ioctl) it freezes — it can no longer run — and its COW pages
//! become an immutable baseline inherited by every descendant. At that instant,
//! before any child clones the EPT, we collapse each private COW page onto a
//! single shared copy. Identical pages across unrelated forks then share one
//! physical page instead of one per fork.
//!
//! ## Page matching
//!
//! - **Zero pages** (by far the most common duplicate) are detected with a
//!   cheap word scan and mapped to a single immortal shared zero page — no hash.
//! - **Other pages** are keyed by CRC64 (PCLMULQDQ-accelerated, 64-bit) and
//!   confirmed with a full `memcmp` before sharing. A hash collision with
//!   differing content simply leaves the page private (a missed dedup, never a
//!   correctness problem).
//!
//! ## Safety / determinism
//!
//! Frozen VMs never run, and children fork with read-only (R+X) EPT clones, so
//! any guest write COWs into a fresh private page first. Shared pages are
//! therefore only ever read, and we only merge byte-identical content, so the
//! merge is invisible to guests and preserves determinism.
//!
//! ## Lifetime
//!
//! A shared page is anchored by the frozen parent that holds a [`SharedRef`] to
//! it (refcounted here). Descendants keep that parent alive via their `_parent`
//! Arc, so the page outlives every EPT leaf that points at it without a reverse
//! map. The page is freed when its last [`SharedRef`] is released (the zero page
//! is immortal).

use core::pin::Pin;

use kernel::alloc::flags::GFP_KERNEL;
use kernel::page::Page;
use kernel::rbtree::{RBTree, RBTreeNodeReservation};

use super::memory::{HostPhysAddr, VirtAddr};
use super::page::KernelPage;
use super::vmx::traits::Page as PageTrait;
use super::vmx::PageDeduplicator;

/// Guest page size in bytes.
const PAGE_SIZE: usize = 4096;

/// A canonical (shared) page held by the store.
struct Canonical {
    /// The owning kernel page. Never read directly — held so the page stays
    /// allocated and is freed when this `Canonical` is dropped.
    #[allow(dead_code)]
    page: Page,
    phys: HostPhysAddr,
    virt: VirtAddr,
    /// Number of live [`SharedRef`]s pointing at this page.
    refcount: usize,
}

/// A refcounted handle to a shared page. Plain data (no raw pointers): released
/// on drop by looking the canonical back up under the store lock.
pub(crate) enum SharedRef {
    /// The single shared all-zero page.
    Zero,
    /// A content page in `entries`, keyed by its CRC64.
    Hashed(u64),
}

/// Global content-addressed page store.
struct CowStore {
    /// Non-zero content pages, keyed by CRC64. Hash collisions with differing
    /// content are not deduplicated (the colliding page stays private).
    entries: RBTree<u64, Canonical>,
    /// The single shared all-zero page (immortal once allocated).
    zero: Option<Canonical>,
    /// Live [`SharedRef`]s handed out (sum of refcounts).
    refs: u64,
    /// Distinct canonical pages currently live (includes the zero page).
    canonical: u64,
}

// SAFETY: `CowStore`/`Canonical` hold a kernel `Page` (Send) and plain data; all
// access is serialized by the `STORE` mutex below.
unsafe impl Send for CowStore {}

// Global store mutex.
// SAFETY: Initialized in module init before first use.
kernel::sync::global_lock! {
    unsafe(uninit) static STORE: Mutex<Option<CowStore>> = None;
}

/// Initialize the global store. Must be called once during module init.
pub(crate) fn init() {
    // SAFETY: called exactly once during module initialization, before any
    // other access to STORE.
    unsafe {
        STORE.init();
    }
    let mut guard = STORE.lock();
    *guard = Some(CowStore {
        entries: RBTree::new(),
        zero: None,
        refs: 0,
        canonical: 0,
    });
}

/// True if the 4KiB page at `virt` is entirely zero. Word-at-a-time with early
/// exit, so non-zero pages almost always bail on the first word.
fn is_zero_page(virt: VirtAddr) -> bool {
    let words = virt.as_u64() as *const u64;
    for i in 0..(PAGE_SIZE / 8) {
        // SAFETY: `virt` is a live 4KiB kernel page; reading 512 u64s is in-bounds.
        if unsafe { *words.add(i) } != 0 {
            return false;
        }
    }
    true
}

/// CRC64 (NVME, PCLMULQDQ-accelerated) of the 4KiB page at `virt`.
fn crc64_page(virt: VirtAddr) -> u64 {
    // SAFETY: `virt` is a live 4KiB kernel page; the helper only reads it.
    unsafe {
        super::c_helpers::bedrock_crc64(
            virt.as_u64() as *const core::ffi::c_void,
            PAGE_SIZE,
        )
    }
}

/// View the 4KiB page at `virt` as a byte slice (for content comparison).
///
/// # Safety
/// `virt` must point to a live 4KiB kernel page that stays mapped for the
/// lifetime of the returned slice.
unsafe fn page_bytes<'a>(virt: VirtAddr) -> &'a [u8] {
    unsafe { core::slice::from_raw_parts(virt.as_u64() as *const u8, PAGE_SIZE) }
}

/// Release a shared reference, freeing the canonical page if it was the last
/// (the zero page is immortal and never freed).
pub(crate) fn release(sref: SharedRef) {
    let mut guard = STORE.lock();
    let store = match guard.as_mut() {
        Some(s) => s,
        None => return,
    };
    match sref {
        SharedRef::Zero => {
            if let Some(z) = &mut store.zero {
                if z.refcount > 0 {
                    z.refcount -= 1;
                }
            }
            if store.refs > 0 {
                store.refs -= 1;
            }
        }
        SharedRef::Hashed(hash) => {
            if let Some(canon) = store.entries.get_mut(&hash) {
                if canon.refcount > 0 {
                    canon.refcount -= 1;
                }
                let now_zero = canon.refcount == 0;
                if store.refs > 0 {
                    store.refs -= 1;
                }
                if now_zero {
                    let _ = store.entries.remove(&hash);
                    if store.canonical > 0 {
                        store.canonical -= 1;
                    }
                }
            }
        }
    }
}

/// Log the current global deduplication picture after a VM has been folded in.
pub(crate) fn log_stats(vm_id: u64, examined: usize, shared: usize) {
    let guard = STORE.lock();
    let store = match guard.as_ref() {
        Some(s) => s,
        None => return,
    };
    // `refs - canonical` is the number of page copies eliminated by sharing.
    let reclaimed = store.refs.saturating_sub(store.canonical);
    let (cg, cf) = to_gib(store.canonical);
    let (rg, rf) = to_gib(store.refs);
    let (sg, sf) = to_gib(reclaimed);
    log_info!(
        "DEDUP: VM {} examined {} pages, shared {}; store: canonical={} pages ({}.{:02} GiB), refs={} ({}.{:02} GiB), reclaimed={} pages ({}.{:02} GiB)\n",
        vm_id,
        examined,
        shared,
        store.canonical,
        cg,
        cf,
        store.refs,
        rg,
        rf,
        reclaimed,
        sg,
        sf
    );
}

/// Log the store's residual size. Call at module unload to diagnose leaks: after
/// all VMs drop this should be 0 except for the single immortal zero page.
pub(crate) fn log_teardown() {
    let guard = STORE.lock();
    if let Some(store) = guard.as_ref() {
        let (cg, cf) = to_gib(store.canonical);
        log_info!(
            "DEDUP teardown: store still holds canonical={} pages ({}.{:02} GiB), refs={} (expect canonical<=1, the immortal zero page)\n",
            store.canonical,
            cg,
            cf,
            store.refs
        );
    }
}

/// GiB (whole, hundredths) for `pages` 4KiB pages, integer math.
fn to_gib(pages: u64) -> (u64, u64) {
    let bytes = pages * (PAGE_SIZE as u64);
    (bytes >> 30, ((bytes & ((1 << 30) - 1)) * 100) >> 30)
}

/// The kernel module's deduplication strategy: interns pages into [`STORE`].
pub(crate) struct KernelDedup;

impl PageDeduplicator<KernelPage> for KernelDedup {
    fn dedup_page(&self, page: &mut KernelPage) -> HostPhysAddr {
        let orig_phys = page.physical_address();
        let orig_virt = page.virtual_address();

        // Read-only classification happens before taking the store lock.
        let zero = is_zero_page(orig_virt);
        let hash = if zero { 0 } else { crc64_page(orig_virt) };

        let mut guard = STORE.lock();
        let store = match guard.as_mut() {
            Some(s) => s,
            None => return orig_phys,
        };

        // Zero-page fast path.
        if zero {
            if let Some(z) = &mut store.zero {
                z.refcount += 1;
                let (zp, zv) = (z.phys, z.virt);
                store.refs += 1;
                drop(page.take_owned()); // free the redundant private page
                page.set_shared(SharedRef::Zero, zp, zv);
                return zp;
            }
            // First zero page seen: donate it as the immortal canonical.
            let owned = match page.take_owned() {
                Some(p) => p,
                None => return orig_phys,
            };
            store.zero = Some(Canonical {
                page: owned,
                phys: orig_phys,
                virt: orig_virt,
                refcount: 1,
            });
            store.refs += 1;
            store.canonical += 1;
            page.set_shared(SharedRef::Zero, orig_phys, orig_virt);
            return orig_phys;
        }

        // Existing candidate with the same content hash?
        if let Some(canon) = store.entries.get_mut(&hash) {
            // SAFETY: both pages are live 4KiB kernel pages held under this lock.
            let equal = unsafe { page_bytes(canon.virt) == page_bytes(orig_virt) };
            if equal {
                canon.refcount += 1;
                let (cp, cv) = (canon.phys, canon.virt);
                store.refs += 1;
                drop(page.take_owned()); // free the redundant private page
                page.set_shared(SharedRef::Hashed(hash), cp, cv);
                return cp;
            }
            // Hash collision with different content: leave the page private.
            return orig_phys;
        }

        // Miss: this page becomes the canonical copy. Reserve the tree node
        // first, so an allocation failure leaves the page untouched (private).
        let reservation = match RBTreeNodeReservation::<u64, Canonical>::new(GFP_KERNEL) {
            Ok(r) => r,
            Err(_) => {
                log_warn!(
                    "DEDUP: node alloc failed; page {:#x} stays private\n",
                    orig_phys.as_u64()
                );
                return orig_phys;
            }
        };
        let owned = match page.take_owned() {
            Some(p) => p,
            None => return orig_phys,
        };
        let node = reservation.into_node(
            hash,
            Canonical {
                page: owned,
                phys: orig_phys,
                virt: orig_virt,
                refcount: 1,
            },
        );
        // Key is fresh (get_mut returned None above), so this inserts a new node.
        let _ = store.entries.insert(node);
        store.refs += 1;
        store.canonical += 1;
        page.set_shared(SharedRef::Hashed(hash), orig_phys, orig_virt);
        orig_phys
    }
}
