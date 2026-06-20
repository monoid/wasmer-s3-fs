# Design doc

## Prior art

The design space for "a filesystem over object storage" spans a spectrum from
"keep everything in the blob store" to "keep metadata in a separate database":

* **Network filesystems (NFS, etc.)** — the latency baseline this system aims to
  beat. They assume a low-latency, mutable backing store; object storage is
  neither, so the data model has to be rethought rather than ported.
* **S3-as-a-filesystem gateways (`s3fs`, `goofys`, `mountpoint-s3`)** — map paths
  directly to keys. Simple single-file access, but directories are emulated via
  `LIST`, and renames are non-atomic copies. This is essentially Alternative 2.
* **JuiceFS** — the canonical "metadata elsewhere" design: file *data* is chunked
  into an object store, while *metadata* (the tree, atomic renames, locks) lives
  in a separate strongly-consistent database (Redis / TiKV / FoundationDB / …).
  It is the living example of the external-coordination tradeoff in
  Alternative 4: fast and simple metadata operations, at the price of a second
  system that is its own point of failure. Our design deliberately sits at the
  opposite end — everything in S3 — to maximize availability.
* **Content-addressed (Git, and Git-like layouts on blob storage)** — immutable
  nodes keyed by content hash plus a single mutable root pointer. This is the
  root-pointer alternative referenced in the [Concurrency
  model](#concurrency-model): atomic whole-tree commits and free snapshots, at
  the cost of root contention.

This design is closest in spirit to "everything in S3" while borrowing the
inode-indirection idea (uuid object names) from traditional filesystems to keep
renames cheap.

## Alternatives
1. *Storing the directory structure elsewhere.* It would add another point of
   failure to the system. While some additional service (DB) may be useful for
   caching and update coordination, storing all the info in S3 would allow
   access to the data even if that another system is not available.
2. *Using paths (or their hashes) as S3 keys*.  It would make it much simpler to
   access a single file, but renames would be more challenging and non-atomic.
3. *Storing file content as sequence of blocks.*  It would allow partial
   updates, but the implementation is much more complex.
4. *External coordination (etcd / ZooKeeper / FoundationDB).* Instead of
   building locking and concurrency control on top of S3 conditional writes, a
   dedicated coordination service could provide them directly. This is a real
   simplification of the *coordination code*, but it trades against two
   properties this task prizes ("minimize state outside S3", resilience), so it
   is an alternative rather than the chosen design.

   What it buys:
   * **Leases that expire on client death** (etcd leases, ZooKeeper ephemeral
     nodes) — no home-grown S3 `lock` object with a TTL to renew.
   * **Fencing for free** via monotonic revisions (`mod_revision`, `zxid`),
     i.e. the "stalled lock holder wakes up and writes" hazard is handled.
   * **Change notifications (watches)** — efficient *multi-writer* caching needs
     cache invalidation when another writer mutates a directory, and S3 has no
     low-latency push (its event notifications are async, via SQS/SNS/Lambda).

## The idea

For simplicity both file data and the directory tree is stored in single S3 bucket.

Each entity (dir or file) is stored with a name of format "`{type}_{uuid4}`",
where type is either "d" or "f" (other types are not considered). This name is
called "object name". The root node has fixed object name "d_root".

The "d" object (directory) is a JSON (for simplicity) object containing a
mapping from path component to certain metainformation, including the object name:

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

CAS updates are used to modify the directories, but for certain mutation
operations an external coordinator would be useful.

### Updating

Concurrent update of the S3 can be a challenge when we consider changes at the
different levels.  However, operations like creating and removing items can be
implemented with S3 CAS aka optimistic locking.  The full concurrency story —
how the same model serves both single-writer and multi-writer modes — is
described in the [Concurrency model](#concurrency-model) section below.

#### Removing directory

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

The tombstone is the durable **commit point**: a crash after
step 1 leaves `D` logically deleted, with steps 2–3 finished by recovery/GC.

The same pattern generalizes: any operation whose logical commit lives in a
*different* object than the one it must exclude against has to move its
compare-and-swap onto the contended object (or take a lock). `rename` above is
the harder instance, where there are two such objects.
 
### Writing to a file

To adapt the `AsyncWrite` interface to the S3 model, S3's multipart upload
mechanism is used.  S3's object is made visible only when the upload is
complete, so no partial write is visible.

The system doesn't support partial updates.  A written file can be only written
entirely.  However, inserting a creates a race condition too: the directory
can be removed by the time the update completes.  The solution is to create
a dummy entry ("intention") that would prevent it.

There is also a fault-tolerance problem: if the writer crashes after the upload,
the intention may stuck forever.  It can be solved with TTL for the intention
that writers may update from time to time if needed.

Writing to the file also adds a race condition: a writer may upload the file
object but crash before linking it into its parent directory, leaving an
unreferenced object behind.  Similar two-phase solution can be implemented here,
making failing write detectable and correctable.

#### Renames

Currently only renames within the same directory is supported as they are
touching only single directory object.

Cross-directory multi-writer renames require either external coordination or
complex "intention"-like scheme.

#### Reading

Readers simply walk the directory tree without any locking or CAS.

Reading from a file is straightforward with range. Some buffering may be
beneficial, but it is out of scope of this simple implementation.

Reading a nested path requires several S3 requests; an application may implement
caching with all the caveats caching has.

### Limitations
1. Only creation time is supported.  Neither modification nor access time is
   supported.
2. Only new file can be written to, and only entirely, no file updates.
3. Large dirs can be quite slow to read and update.  Specially designed entry
   file format may allow partial range reads to read it faster.
4. Async file reading and writing is implemented with a blocking s3 client.
5. The current solution is not fault-tolerant.  Some modifications
   ("intentions") with an external ACID storage should be used.

## Concurrency model

### Multi-writer mode

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

* **Same-directory `rename` is trivial** and is the only case the prototype
  implements so far: source and destination share one parent, so moving the
  entry from one key to another is a single CAS on that one directory object —
  atomic by construction, with no intent needed. Concurrent mutations of the
  same directory serialize on that CAS like any other update. (The prototype
  rejects an existing destination with `AlreadyExists` rather than overwriting
  it; POSIX-style replace would additionally have to reclaim the displaced
  object via a tombstone + GC.)

* **Cross-directory `rename` is the hard case.** It touches two directory
  objects (remove the entry from source `A`, add it to destination `B`, both
  pointing at the same object name) and S3 has no multi-object transaction.

  The cleanest approach generalizes the deletion tombstone: store the
  **intent in the participant nodes themselves**. The tombstone is just the
  degenerate, single-participant case of this ("this directory is being
  deleted"); `rename` is the two-participant case. A separate "intent marker"
  object would also work, but it would have to be reconciled with the
  directories separately, whereas an in-node intent is serialized by the very
  same CAS that guards the entries it concerns.

  An external per-directory lock (taken on `A` and `B` in a canonical order to
  avoid deadlock) is an alternative way to exclude *concurrent* writers, but it
  does **not** remove the need for the intents: a lock does not survive a crash
  mid-saga, so recovery still relies on the in-node `pending`/`from` records.
  Lock + intents together is the simplest fully-correct combination. (Note that
  the root-pointer alternative sidesteps all of this — `rename` there is one
  atomic CAS on the root — which is the price/benefit tradeoff of that model.)

* **Readers never take any lock** in either mode — they just read tree objects
  (optionally cached and validated by ETag).

### Resilience and read-only fallback

Because readers are lock-free and read directly from S3, the read path stays
available even when the write plane is degraded. A writer that loses its lease
(or cannot renew it due to connectivity issues) **downgrades to read-only**
rather than risking split-brain. This gives the required "fallback read-only
mode" with no extra machinery.
