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
implemented with S3 CAS aka optimistic locking.

### Writing to a file

To adapt the `AsyncWrite` interface to the S3 model, S3's multipart upload
mechanism is used.  S3's object is made visible only when the upload is
complete, so no partial write is visible.

Writing to the file also adds a race conditions: a writer may upload the file,
but fail to update the dir due to a crash.  It can be solved by adding the
upload info to some database, and special service could remove 

### Reading from a file

Reading from a file is rather straightforward with range. Some buffering may be
beneficial, but it is out of scope of this modest implementation.

### Limitations
1. Only creation time is supported.  Neither modification nor access time is
   supported.
2. Only new file can be written to, and only entirely, no file updates.
3. Large dirs can be quite slow to read and update.  Specially designed entry
   file format may allow partial reads to read it faster.

## Alternatives
1. *Storing the directory structure elsewhere.* It would add another point of
   failure to the system. While some additional service (DB) may be useful for
   caching and update coordination, storing all the info in S3 would allow
   access to the data even if that another system is not available.
2. *Using paths (or their hashes) as S3 keys*.  It would make it much simpler to
   access a single file, but renames would be more challenging and non-atomic.
3. *Storing file content as sequence of blocks.*  It would allow partial
   updates, but the implementation is much more complex.
