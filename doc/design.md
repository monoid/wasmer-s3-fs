# Design doc

## The idea

For the better availability, both file data and the directory tree is stored in
single S3 bucket.

Each entity (dir or file) is stored with a name of format "`{type}_{uuid4}`",
where type is either "d" or "f" (other types are not considered).  This name is
called "object name".

The "d" object (directory) is a JSON (for simplicity) object containing a
mapping from path component to certain metainformation, including the object name:

The root node has fixed object name "d_root".

```
{
  "children": {
     "home": {
         "obj_name": "d_1234asf"
         "ctime": 1234,
         "len": 0
     },
     "usr": {
         "obj_name": "d_8323klm"
         "ctime": 2343,
         "len": 0
     }
  }
}
```

### Updating

Concurrent update of the S3 can be a challenge when we consider changes at the
different levels.  However, operations like creating and removing items can be
implemented with S3 CAS aka optimistic locking.  The full concurrency story —
how the same model serves both single-writer and multi-writer modes — is
described in the [Concurrency model](#concurrency-model) section below.

### Writing to a file

To adapt the `AsyncWrite` interface to the S3 model, S3's multipart upload
mechanism is used.  S3's object is made visible only when the upload is
complete, so no partial write is visible.

Writing to the file also adds a race condition: a writer may upload the file
object but crash before linking it into its parent directory, leaving an
unreferenced object behind.  Rather than tracking in-flight uploads in an
external database (which would add state outside S3), we rely on the
*data-before-pointer* invariant plus an age-based garbage collector; see
[Garbage collection](#garbage-collection).

### Reading from a file

Reading from a file is rather straightforward with range. Some buffering may be
beneficial, but it is out of scope of this modest implementation.

### Limitations
1. Only creation time is supported.  Neither modification nor access time is
   supported.
2. Only new file can be written to, and only entirely, no file updates.
3. Large dirs can be quite slow to read and update.  Specially designed entry
   file format may allow partial reads to read it faster.

## Concurrency model

The task asks for two modes: a *single-writer* mode that locks the whole volume,
and a *multi-writer* mode that should be "at least somewhat efficient" (the
latter only has to be **designed**, not implemented).

The key idea is that both modes run on **one and the same on-S3 data model**
described above (the inode tree of CAS-updatable directory objects). They differ
only in the *granularity of locking*, not in the data layout. Multi-writer is
therefore a first-class concern of the core design, and single-writer is a
performance specialization layered on top of it.

### Shared substrate: CAS-updatable objects + a lease lock

Two primitives are enough to build everything:

1. **Per-object compare-and-swap.** Every directory mutation is a
   read-modify-write guarded by a conditional `PUT` (`If-Match: <etag>` for an
   update, `If-None-Match: *` for a create). A lost CAS race surfaces as `412
   PreconditionFailed`, which `update_dir` treats as a retry signal: it reloads
   the object and re-runs a *pure* transform closure until the conditional write
   commits. (This loop is implemented in the prototype.)

2. **A volume lease lock.** A single `lock` object created with
   `If-None-Match: *` (create-if-absent) holding `{owner_id, expiry,
   fencing_token}`. The holder renews the lease periodically; on a crash the
   lease expires and another writer may take over. The monotonic *fencing token*
   guards against the classic "stalled holder wakes up and writes" hazard: every
   write carries the token and a stale writer's conditional writes fail.

No state lives outside S3: the lock, the tree, and the data are all objects in
the bucket.

### Multi-writer mode (first-class design)

No volume lock is taken; writers coordinate purely through per-object CAS.

* **Parallelism comes for free across directories.** Because each directory is
  its own object, two writers mutating `/a` and `/b` touch different objects and
  never conflict. Contention only arises *within a single hot directory*. This
  is precisely why the per-directory-object layout (rather than, say, a single
  root-pointer object — see Alternatives) is a good fit for efficient
  multi-writer: independent subtrees scale out.

* **Hot-directory contention** is mitigated by *sharding* a directory across N
  sub-objects keyed by a hash prefix of the entry name, so concurrent creates in
  the same logical directory land in different shards. The tradeoff is that a
  listing becomes N `GET`s instead of one.

* **Cross-directory `rename` is the hard case.** It touches two directory
  objects and S3 has no multi-object transaction. Options, in increasing order
  of cost:
  1. Treat `rename` as the one operation that still takes a short lock — acquire
     per-directory leases on both directories in a canonical order (sorted by
     object name, to avoid deadlock). `rename` is rare, so a brief lock is
     acceptable.
  2. Record an *intent marker* object ("pending rename from X to Y") before
     touching either directory, so a crashed rename can be rolled forward or
     back by recovery. This costs a little extra (transient) state in S3.
  3. Simply document the non-atomicity and the recovery invariant.

* **Garbage collection must be lock-free, and it is** (see "Garbage collection"
  below): the age-based mark-and-sweep needs no coordination with live writers,
  which is exactly what a multi-writer deployment requires.

* **Readers never take any lock** in either mode — they just read tree objects
  (optionally cached and validated by ETag).

### Two-phase directory deletion

Removing a directory is a second multi-object hazard, subtler than rename and
worth spelling out because it is easy to get wrong. Naively, `remove_dir(D)`
would check that `D`'s object is empty, then unlink the entry for `D` from its
**parent** and delete `D`'s object. But the emptiness check reads `D` while the
commit is a CAS on the *parent* — two different objects. A concurrent
`create_dir(D/x)` commits via a CAS on **`D`**, so it does not conflict with the
parent CAS. The interleaving

> remove reads `D` empty → create inserts `x` into `D` → remove unlinks and deletes `D`

silently loses the freshly created `x`.

The fix is to move the deletion's commit point onto **`D` itself**, so it
contends with inserts into `D`:

1. **Seal** `D` by CAS-writing a `deleted` tombstone onto its own object,
   conditional on it still being empty.
2. **Unlink** the entry for `D` from the parent.
3. **Delete** `D`'s object.

Now removal and a racing insert both CAS the same object `D`:

* if the tombstone lands first, the insert's retry reloads `D`, sees the
  tombstone, and is refused;
* if the insert lands first, the tombstone attempt reloads `D`, sees it
  non-empty, and fails with `DirectoryNotEmpty`.

A tombstoned directory is treated as absent by every operation (loading it
returns "not found"), so inserting into a directory that is being deleted fails
automatically. The tombstone is the durable **commit point**: a crash after
step 1 leaves `D` logically deleted, with steps 2–3 finished by recovery/GC.
(One residual window: between steps 1 and 2 an operation that inspects the
*parent's* entry for `D` rather than loading `D` — e.g. `metadata` — may still
see it briefly; closing that fully needs the recovery sweep to complete the
unlink.)

The same pattern generalizes: any operation whose logical commit lives in a
*different* object than the one it must exclude against has to move its
compare-and-swap onto the contended object (or take a lock). `rename` above is
the harder instance, where there are two such objects.

### Single-writer mode (performance specialization)

A writer takes the volume lease and is then the *only* mutator of the tree.
This collapses most of the difficulty:

* No per-operation CAS contention, no cross-directory rename race — there are no
  competing writers, only crash recovery (handled by lease expiry + GC).
* The holder can keep an **authoritative in-memory, write-back cache of the
  directory tree**. Path resolution becomes local: walking `/a/b/c/d/file` costs
  *zero* round-trips instead of one `GET` per component. This is the main lever
  for being "faster than NFS": the cache is trivially valid precisely *because*
  the lease guarantees no one else mutates the tree.
* Mutations can be batched and flushed lazily, amortizing S3 round-trips.

So single-writer is not a different system — it is the same data model with the
lock granularity turned up to "whole volume", which unlocks aggressive caching.

### Resilience and read-only fallback

Because readers are lock-free and read directly from S3, the read path stays
available even when the write plane is degraded. A writer that loses its lease
(or cannot renew it due to connectivity issues) **downgrades to read-only**
rather than risking split-brain. This gives the required "fallback read-only
mode" with no extra machinery.

### Garbage collection

Partial failures leave two kinds of garbage, both collected without
stop-the-world and without state outside S3:

1. **Incomplete multipart uploads** (a writer started an upload but never
   completed it) are cleaned by an S3 bucket lifecycle rule
   (`AbortIncompleteMultipartUpload after N days`) — a native mechanism, no code.

2. **Completed-but-unreferenced objects** (a writer finished `f_X` but crashed
   before linking it into a directory) are collected by an *age-based*
   mark-and-sweep:
   * Invariant: an object is always written *before* it is referenced
     (data-before-pointer). This also guarantees readers never observe a
     dangling reference.
   * Rule: an object is deletable only if it is **both** unreachable from the
     root **and** older than a grace period `T`, where `T` exceeds the maximum
     time an in-flight operation can take between "object created" and "object
     linked".
   * Why it is race-free: a freshly uploaded but not-yet-linked object is younger
     than `T`, so the sweeper skips it; by the time it is older than `T` it has
     either been linked (now reachable) or the linking operation has definitely
     failed (a true orphan). No writer needs to be paused.

   With the root-pointer alternative, GC can instead walk an *immutable snapshot*
   of the current root (Git-style `gc` with a prune grace period), which makes
   the reachability walk race-free by construction.

3. **Half-deleted directories** — a `remove_dir` that sealed a directory with a
   `deleted` tombstone (step 1 above) but crashed before unlinking and deleting
   it (steps 2–3). Recovery completes the protocol: a tombstoned directory
   object is, by definition, logically removed, so the sweep finishes the
   pending unlink from its parent and then deletes the object. Because a
   tombstoned directory is invisible to normal operations, leaving the work to a
   later sweep is safe.

## Alternatives
1. *Storing the directory structure elsewhere.* It would add another point of
   failure to the system. While some additional service (DB) may be useful for
   caching and update coordination, storing all the info in S3 would allow
   access to the data even if that another system is not available.
2. *Using paths (or their hashes) as S3 keys*.  It would make it much simpler to
   access a single file, but renames would be more challenging and non-atomic.
3. *Storing file content as sequence of blocks.*  It would allow partial
   updates, but the implementation is much more complex.
