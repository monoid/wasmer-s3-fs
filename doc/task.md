# Hiring Task - Distributed Filesystem

Good morning, development ninja.
Your mission, should you choose to accept it:

## Background

Wasmer needs a distributed file system to power the Wasmer Edge platform.

It must:

- Use object storage (like `S3`) as the underlying storage mechanism
    - S3 supports `CAS` (compare and swap) operations, which are technically sufficient to implement advanced logic
- Support a single writer mode, taking a lock on the filesystem volume
- Support a multi-writer mode at least somewhat efficiently
(note: this is not required to be implemented, just considered in the core design)
- Be fast enough for varied use cases (faster than NFS, assuming an S3 service inside the same cluster)
- Be reasonably resilient to network connectivity issues, eg with a fallback read-only mode
- Avoid incidental complexity
- Minimize state that must be maintained outside of S3 (within constraints imposed by performance considerations)

**Note**: this is a very free-form task. The design space is huge.

You are *not* expected to build  complete, comprehensive knowledge of the “distributed systems” design space or deliver a polished prototype.
The focus should be on:

- Investigating the design space
- Coming up with a viable design
- Delivering a rough but functional prototype

Feel free to make omissions as appropriate to keep the scope manageable. 

The solution would later be integrated into the WASIX file system abstraction layers
You can optionally  implement the relevant `Filesystem` traits of the `virtual-fs` crate in the http://github.com/wasmerio/wasmer repository.
This may be helpful for ensuring the required functionality is represented.

## Deliverables

- a design document that outlines
    - Design space & prior art
    - Chosen implementation design, with pros and cons/tradeoffs
    - Alternatives
    - Explanation of the prototype
    - How the draft design and implementation could be evolved into a fully fleshed out system
- functional prototype in Rust
    - Supports basic file system functionality
    - small test suite
    - Can be built against a local self-hosted S3 compatible service  (like `minio` )
    

> We will evaluate submissions based on design judgment, correctness of the core model, clarity of tradeoffs, prototype usability, test coverage, simplicity, and how well the design could evolve toward production use.
> 
