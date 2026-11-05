# Complexity

## Don't manage complexity—refuse to create it.

- Make the illegal state unrepresentable.
- Push complexity to the edges so the core stays simple.
- Delete the requirement before you optimize the solution.
- Choose constraints early so you don't pay for freedom you never use.
- The cheapest code to maintain is the code you didn't write.
- Collapse the problem space before you search it.

My favorite of the set is probably "make the illegal state unrepresentable" — it's the type-system version of refusing to create complexity, which fits your Rust work especially well.

Make the illegal state unrepresentable - there is a mountain of work to make that possible

Right—the one-liner hides the cost. The payoff is real but it's front-loaded and it compounds against you in three ways:

The modeling tax. Encoding invariants in types means doing the domain analysis before you write logic. You have to actually know which states are legal, which is the hard part most people defer. Newtypes, typestate, sealed enums, NonZero, phantom markers—each is cheap individually, but discovering the right algebra of your domain is the mountain.

The friction at the boundary. Illegal states sneak in at I/O edges—deserialization, gRPC/protobuf, user input, DB rows. protobuf is the worst offender: every field is optional and scalar-defaulted, so the wire format is a sea of illegal states. You end up writing a parse-don't-validate layer (RawRequest → ValidatedRequest) that does the work of rejecting bad states once, at the door. That conversion code is pure tax, and it's where the modeling actually lives.

The refactor cost. Tightening a type ripples. Splitting one enum into two, or making a field's presence depend on a variant, can touch every call site. The compiler guides you, which is the whole point, but "the compiler will tell you everywhere you were wrong" is also a euphemism for a long afternoon.

The honest framing: it's not "make illegal states unrepresentable," it's make the expensive-to-debug illegal states unrepresentable. You're trading construction-time effort for the entire class of runtime bugs and defensive checks you'd otherwise carry forever. For a long-lived system with distributed sync and air-gapped deploys where you can't just hotfix, the trade is overwhelmingly worth it. For a throwaway script it's malpractice.

The mountain is real. It's just smaller than the mountain of debugging the states you allowed.

`protobuf is the worst offender but it is so good...`

It's both, and that's exactly the tension.

protobuf is great at what it was designed for: a compact, fast, schema-evolved wire format with codegen across a dozen languages. Backward/forward compatibility is the killer feature—add a field, old readers ignore it, new readers default it, nothing breaks. For a distributed system spanning versions and air-gapped deploys that drift, that property is gold.

But that same property is the illegal-state generator. The compatibility guarantee is purchased by making everything optional and defaultable at the wire level. proto3 deliberately erased required fields and (originally) presence tracking because "required is forever"—a required field you later want to drop breaks every old client. So the format pushes all validation up to you, by design. The wire type is permissive precisely so the schema can evolve; the cost is that "valid protobuf" and "valid domain object" are different sets, and the gap is yours to close.

So the resolution isn't "protobuf bad"—it's protobuf is a transport schema, not a domain model. It's doing its job by being permissive. The mistake is letting the generated structs leak into your core as if they were your types. The fix is the boundary layer: prost generates the wire structs, and you convert into your real domain types where presence is resolved, enums are exhaustive, and invariants hold. TryFrom<proto::Foo> for domain::Foo is the idiomatic spot—all the "this should never be None" rejection happens once, returning a typed error.

prost makes this cleaner than most: oneof becomes a real Rust enum (genuinely unrepresentable illegal states for free), and proto3 optional gives you Option<T> so you can distinguish absent from default. Pair it with prost-validate or hand-rolled TryFrom and the tax gets paid in one well-defined place instead of scattered .unwrap_or_default() everywhere.

So yes—it's so good. It's just good at being the wire, not the world.

all the "this should never be None" rejection happens once is the quote, a programmer making a claim that will likely be false?

Yes—that's the tell. "This should never be None" is a programmer asserting a fact about runtime they don't control, which is the exact thing the type was trying to stop them from doing.

But there's a distinction that saves it. There are two places that sentence can live:

At the boundary, in the TryFrom, it's not a claim—it's a check. field.ok_or(Error::MissingName)? doesn't assert the field is present; it handles both cases and rejects one. That's the rejection happening honestly. The None is fully expected there; that's the whole job of the layer.

Past the boundary, in your domain logic, the sentence shouldn't exist at all—because if you did the conversion right, the field isn't an Option anymore. domain::Foo.name is a String, not Option<String>. There's no None to reject because the type can't hold one. The illegal state is unrepresentable, so the claim is unstatable.

The failure mode—the false claim you're pointing at—is the third place: someone keeping the proto struct around past the boundary and writing proto_foo.name.unwrap() // always set in business logic. That is the programmer making a claim that will likely be false, because the wire type permits None and nothing structural stops a future caller, a malformed peer, or a schema change from delivering one. The comment is load-bearing and comments don't load-bear.

So the quote is fine if "happens once" means "is converted into a type where the question can't be asked again." It's a lie if "happens once" means "I checked it that one time and now I'll assume it forever." The discipline is: the boundary is allowed to say None; nothing downstream is allowed to know the word.

Paradoxically, we opt to create "good" complexity in order to avoid "bad" complexity. "Refuse to create complexity" trivializing the challenge: it is a slogan and as a slogan it compresses a real judgment into something that sounds like a rule.

The accurate version isn't "refuse to create complexity." It's spend complexity where it buys leverage, refuse it where it doesn't. The boundary layer is complexity you author deliberately, in one place, to retire a whole category of complexity you'd otherwise pay everywhere and forever. That's not avoidance—it's concentration. You're not removing the difficulty; you're choosing its location, its shape, and who has to look at it.

So the dimension that actually matters isn't amount, it's distribution. Bad complexity is diffuse, ambient, load-bearing-by-accident—the .unwrap() in business logic, the comment that has to be true, the invariant maintained by everyone remembering. Good complexity is localized, named, and total—the TryFrom that owns the question so nothing else has to ask it. Same problem, same irreducible hardness; one version smears it across the codebase, the other corrals it. "Refuse to create complexity" really means "refuse to smear it."

There's a conservation law underneath, which is why the paradox feels real: essential complexity can't be destroyed (Brooks' point about no silver bullet—the difficulty is in the domain, not the tools). You can only relocate it. So every "simplification" is actually a transfer. The skill is moving it from places that multiply it (every call site, every future maintainer, every runtime) to a place that contains it (one module, compile time, one author). The type system is a complexity sink: it lets you pour the hard part into the boundary and have the compiler hold it there.

So yes, the one-liner trivializes it—deliberately, the way all good aphorisms do. It's a true thing to aim at and a false thing to believe. The work was never to have no complexity. It was to make sure the complexity you have is the kind that pays rent.

Which loops us back to original instinct: "create a web of knowledge" is additive, diffuse, and insufficiently precise, "avoid the problem" is naïve.
Aim to make the hard part exist in exactly one place and make every other place unable to recreate it. 
That's the non-trivial one-liner hiding inside the trivial one.
