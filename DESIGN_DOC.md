# Design document

This file contains a poorly organized collection of thoughts on `salvia`'s internal structure, design choices
and potential for future features.

## Internal structure

### Synchronization

Runtime works in *frames*.
Every frame has **three** looping stages:

1. *Mutation* phase.
    This is an external phase.
    It is indicated to user by access to `InputContext`.
    
    During the phase user is permitted to mutate inputs.
    
    The phase is lazy, i.e. runtime will stay unless prompted to leave.
2. *Actualization* phase.
    This is an internal bookkeeping phase.
    User cannot observe it or interact with runtime while it's in effect.
    
    During the phase clock task prompts all known inputs to actualize their value,
    as well as report if any change has occurred.
    This information is later embedded into `QueryContext`.

    The phase is eager and will roll into next one as soon as it's done.
3. *Query* phase.
    This is an external phase.
    It is indicated to the user by access to `QueryContext`.

    During the phase user is permitted to run queries.

    The phase is lazy, i.e. runtime will stay unless prompted to leave.

There is no special work to be done to transition from *query* to *mutation*.
New frame starts at the beginning of *mutation* phase (which corresponds to *global time* increment).

Initially runtime starts in *query* phase.
Values produced by means of `Input::initial()` function are always considered to be produced in the very first frame
(*global time* 0).

The mental model here is similar to `RwLock`: runtime can only be in either "read" or "write" state at the same time,
everything else (actualization) happens during transition.
Each individual write (`Runtime::mutate` call) is atomic but their relative order is not specified.

### Caching and tasks

Framework data is spread over three levels:

1. Node - manages cache, corresponds to a function with a set of parameter values.
2. Query/input manager - manages nodes as well as holds accessors to them, corresponds to a (monomorphized) function. 
3. Runtime  - holds accessors to all query managers.

Currently, every node resides in a separate task, query managers are merged into runtime.

This arrangement has certain benefits:

* Nodes can directly reference (and access) other nodes as their dependencies.
* Node as separate entity provides very good opportunity for type erasure:
  it can be invoked without providing any arguments.

The only obstacle on the goal of complete type erasure is return type;
that is solved by heuristic: tasks remember *when* value was produced instead of value itself.
Complete type erasure makes querying for updates much more straightforward -
all information exchanged between tasks is independent of types processed by nodes.

Merge of runtime and query managers elides extra channel communication, but is in the way for memory control.

### `Hash` bound

I experimented with providing alternatives (`BTreeMap` and [`LinearMap`][docs.rs:linear-map] for no bound at all),
but it was *very* messy.
The worst part was managers: all implementors of `Input` trait are forced to choose which trait to use with no
ability to provide default.

As already stated in crate documentation, there is little reason to think that `Hash` cannot be implemented
on relevant types, so I deemed support for alternatives unwarranted.

[docs.rs:linear-map]: https://docs.rs/linear-map/latest/linear_map/

## Alternatives

### Caching and tasks

There are other possible arrangements for framework data.
One of the more promising is

1. Merge nodes into query managers on a single task.
2. Keep accessors to those tasks inside runtime.

Attractive points:
*   Much better hold on nodes: any access to queries goes through manager.
    Opens way for relatively easy memory management.
*   Fewer synchronized operations for retrieving value.
    Fewer channels in general.
*   Potentially better memory utilization, fewer indirections.
*   Potential benefits for serialization/deserialization (?).

Bad stuff:
* Loss of type erasure.
    All dependants are now forced to remember input to queries they execute in order to perform updates.
    There are some mitigations possible, like assigning hash to set of input values and
    internally operating on hashes instead.
* Loss of parallelization.
    Mitigation is possible but complex.
* Without scoped tasks potentially extra clones.

There are less drastic approaches, for ex. just separating managers into individual tasks,
but any changes in the area will probably be guided by benchmarking.

## Open questions

### Deserialization

While serialization is mostly a matter of adding an extra trait, deserialization poses significant challenge.

Internally original *function's item type* ([relevant Rust reference][ref:function_type]) is used to discriminate
queries and input's host type to discriminate inputs.
This means during deserialization we need to be able to map from byte sequence to an arbitrary, statically unknown type
somewhere in the program in order to restore state - which is impossible without assistance.

Still, there are three approaches that might work:

* Request user to supply all involved types in order to deserialize them.

    **Pro**: full control on user side.
 
    **Pro**: can potentially handle queries.

    **Con**: most likely requires variadics, i.e. have to be macro-based for the moment.

    **Con**: can get *extremely* verbose.

    **Con**: requires user to have visibility of symbols at the deserialization point.
    This is not enforced, i.e. approach is almost unusable.
    
* Lazy deserialization.
    We preserve serialized data inside runtime and use it to initialize inputs when encountered for the first time.
    
    **Pro**: bypasses visibility restrictions.

    **Con**: incompatible with queries.

    **Con**: lazy; hence extra data being carried around, slower first call to an input.

* Try to remember all query/input types (or rather their deserialization code) at compile time.

    There are a couple crates which can achieve this via link-time shenanigans 
    (see [`linkme`][docs.rs:linkme], [`inventory`][docs.rs:inventory]).

    **Pro**: very clean, no visibility requirements or user intervention, it just works.

    **Pro**: naturally supports queries.
	
    **Con**: limited to single binary.
	
    **Con**: only compatible with concrete functions.
    In order to work with generics it needs to generate a `static` per monomorphization which is currently impossible.
    There are some scattered comments on it on forums and rust's repo issues, but I was not able to find even a pre-RFC.
    This part seems to be hopeless.

For the time being if only input serialization is required user can simply query their values and manually persist data
as necessary.
Deserialization can be achieved by manually setting persisted values before any queries are done.
This approach is functionally equivalent to first method (with caveat that you don't know which inputs where touched
and need persistence).
It could be worth improving on this approach as an alternative. 

[ref:function_type]: https://doc.rust-lang.org/reference/types/function-item.html
[docs.rs:linkme]: https://docs.rs/linkme/latest/linkme/
[docs.rs:inventory]: https://docs.rs/inventory/latest/inventory/

## Future

### Add a parameter to input's `get` and `set`

We cannot support variadics, but it is possible to compress all arguments into single tuple.
This is another ergonomic hit for 0/1-parameter queries.
Maybe as a separate trait/derive?

### Other runtimes

Could be nice to have.
Depends on whether there are ways to abstract over runtime functionality.

### Memory management

It is impossible to evict caches at the moment.
Some sort of automatic cleanup for old data might be desirable.

### Different caching strategies

Only latest produced value is memoized, i.e. caches are effectively LRU with 1 slot.
It could be useful to control that.

### Optimization of `InputSelection` + `InputSnapshot`

For big input sets both can reach unreasonable sizes (multiple KiB and more).
* We likely can expect that most of the time large parts of `InputSelection` are not set - possible optimization?
* We likely can expect that adjacent `InputSnapshot`s share a lot of values - possible optimization?
    It is already shared behind `Arc`, but every frame spawns a new snapshot disregarding data of already existing ones.
    Possible approach: immutable collection?

### Eager queries

Idea behind eager queries is to shadow inputs they depend on.

Queries can depend only on part of big input or on aggregate of inputs. 
Currently, all dependent queries will be flagged for update even when some of them observe no change in relevant data.
Eager queries can provide an optimization for the case, avoiding update cascade.

I experimented with query-like API, it didn't work well.
Input-style trait approach is more promising.

Eager queries are a doorway to (potentially) supporting streams.

### Streams

Effectively equivalent to supporting (deterministic?) state machines.
Also results in a hybrid between push- and pull-based approach - implications?

I believe it may be achievable, but coherence implications, limitations and potential API need more research.
