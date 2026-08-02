# What this program is for, and how we know when it works

Rewritten 2026-07-30, at 0.1.18. The previous version of this file was organised
around version milestones and had become an append-only log with entries out of
order; the news was buried and the numbering had stopped meaning anything.
`CHANGELOG.md` is the log. This document is the *shape*: what the program is, what
"finished" means, how decisions get made here, and what is next. It carries no
version numbers on purpose — none of the remaining work is gated on a release
boundary, and pretending otherwise is how the last version of this file rotted.

## The shape

> A fully automatic RAW developer that replaces the camera's internal
> RAW-to-JPEG processing, but with the computation, memory, and time budget of a
> desktop. Shoot RAW-only on a Samsung S24+ and a Sony A7C, run one command over
> the card or folder, and get archive-ready images for a photo library.
> No per-image adjustment, no editing workflow, no professional ambitions.

Everything below follows from that sentence. It is narrower than a general RAW
developer and the narrowness is the point: a batch archiver with no human in the
loop has different failure modes from an editor, and optimising for the wrong one
is the main way this project could waste effort.

Two consequences worth stating because they are not obvious:

- **There is no user to rescue a bad decision.** An editor can ship a mediocre
  auto mode because a human will nudge it. Here the automatic decision *is* the
  product, so a single ruined frame in a thousand matters more than a hundred
  merely-adequate ones.
- **Compute is nearly free.** The camera had 100 ms on a battery. We have seconds
  on mains power. Any quality gap that exists only because the camera was in a
  hurry is a gap this program has no excuse for.

## Criteria: when it is done

Five conditions. These are the acceptance test; the rest of this document is in
service of them.

1. **One command, no per-source flags.** `raw-autotune <folder> --output <folder>`
   produces correct output for A7C ARW, S24+ Expert RAW, S24+ Pro mode and
   ProShot DNG without the user knowing which is which. A tool for archiving
   cannot require per-file forensics.
2. **Output is library-grade.** The files must behave like camera JPEGs in a photo
   library: correct EXIF (date/time, camera, lens, GPS, orientation), an embedded
   colour profile, a thumbnail-friendly format. For an archiver this is not
   polish, it is the product — without it a library sorts the archive by file
   modification date and shows no capture info.
3. **No frame is ruined.** The bar is not "matches Lightroom Auto", it is "never
   clearly worse than the camera's own JPEG, usually at least as good". No blown
   highlights the camera kept, no crushed shadows, no visibly wrong exposure class
   (night rendered as day), no green campfires. A few merely *bland* results are
   acceptable; ruined ones are not.
4. **High-ISO frames are presentable.** The A7C corpus reaches ISO 12800 and phone
   sensors are noisy at base ISO. The camera always denoises; if this program does
   not, the "bigger compute budget" promise is unmet where it is most visible.
5. **Batch runs are unattended and idempotent.** Point it at a year of RAWs
   overnight: it skips what is already done, survives individual failures, and
   reports what it did. Re-running produces byte-identical output.

### Where they actually stand

| | Criterion | State |
|---|---|---|
| 1 | One command, no flags | **Met.** 376 of 376 corpus files develop from one no-flag invocation, zero failures. The last per-source flag went when the preview oracle became automatic. |
| 2 | Library-grade output | **Met.** EXIF plus a generated sRGB ICC profile in JPEG, TIFF and PNG; verified by reading the tags back and by Windows' own property handlers. `--no-metadata` opts out. |
| 3 | No frame ruined | **Open, and the hard one.** One documented ruin was found and fixed in 0.1.18 — a magenta cast across the shadows of every frame with sub-black samples, invisible to all four scorecard axes and found only by looking. The win/tie/lose scorecard in `docs/STATUS.md` is the instrument; it says highlights are a rout in our favour, shadows a tie, local detail a win. What it cannot yet say is whether any frame is *ruined*, because that needs eyes. See "the grading session" below. |
| 4 | High-ISO presentable | **Half met.** Chroma noise is handled automatically and the extreme-ISO magenta veil is gone. Luma noise is untouched, deliberately — it is the part that destroys texture, and the camera's own high-ISO JPEGs are visibly mushier than ours, so it is not obvious how far to close this. |
| 5 | Unattended and idempotent | **Met.** Skip-existing works, per-file panics are caught so a batch survives, the run reports completed/skipped/failed, and since 0.1.19 `--jobs` defaults to a count worked out from available memory and the largest input rather than to documentation telling the user to pick one. |

## How this project decides things

The durable part. These are the rules that have repeatedly turned out to be right,
usually by first being violated.

### The corpus comes before the tuning

Every conclusion this project drew from a single file was wrong in some detail, and
every conclusion that survived contact with RAW+JPEG pairs was about the exposure
controller rather than the thing being blamed. The "Expert RAW renders 2.6 EV too
bright" defect never generalised — it was the aim-the-median rule failing on night
scenes, visible only once a corpus with references existed.

So: **gather pairs for a scene class before tuning for it, not after.** Shoot
RAW+JPEG while building the corpus; the camera JPEG per frame is what turns "is
ours better?" from an opinion into a measurement.

Remaining corpus gaps, in priority order: snow and other genuinely high-key scenes
beyond the eight already gathered; backlit and high-dynamic-range interiors;
strongly coloured light that must *not* be neutralised (sunset, neon, campfire);
bursts of one scene; and if at all possible a second body or lens, since every A7C
frame here comes from one camera and one photographer.

Also worth having once per source: a grey-card or white-wall frame at each whole
ISO stop, which feeds the pooled noise profile content-free data, and a lens-cap
dark frame at high ISO for a future hot-pixel map.

### The camera JPEG is a reference, not ground truth

The scene classes where this controller is weakest are the same ones where the
vendor is weakest, so the metric is least trustworthy exactly where it is most
needed. On night frames Sony's own rendering is badly underexposed, and the
exposure metric therefore reported a 3.17 EV "error" on frames where our render was
the better one.

**Read the number, then open the pair.**

### Borrow the judgement, beat the rendering

The reference corpus creates an obvious trap: this program borrows the camera's
exposure target and is graded against the camera's JPEG, so it could converge on
being a slower copy of the camera. A camera JPEG contains two separable things:

1. *A judgement about the scene* — that this is a night shot and should look like
   night; where the subject is; what the faces are. That needs having been there,
   plus detection this program defers. Borrowing it via the preview oracle is free
   and caps nothing.
2. *A rendering of that judgement* — one global curve, noise reduction and
   sharpening in about 100 ms on a battery, single pass, tuned to be safe for a
   stranger. Every one of those constraints is absent here.

The oracle borrows only (1): it sets the display EV the key lands on. How the
remaining twelve stops are distributed around it is entirely ours, which is why the
program can sit 0.02 EV from the camera's exposure and still keep more highlight
information on the large majority of frames. Two bounds keep the borrowing honest,
both evidenced: the oracle applies only where the embedded preview is a real
rendering rather than a thumbnail, and `MAX_ORACLE_DEVIATION_EV` stops it following
a vendor rendering that is itself wrong.

Because the oracle is automatic and per file, a pooled scorecard cannot tell "the
controller got better at judging scenes" from "more files happened to carry a
usable preview". So every summary splits its reference measures by `guidance_mode`,
and `--no-preview` renders the independent arm deliberately. **Report both.**

### Grade on axes where "better" needs no reference — and know which those are

Information kept is better and no opinion is required to say so. Taste needs a
reference. The two must not be confused, and this project has now been burned in
both directions:

- `clipped_fraction` and `crushed_fraction` are genuinely one-directional.
- `luminance_entropy` and `average_gradient` are **not**, despite looking like it.
  Their maximisers are histogram equalisation and amplified noise respectively. A
  `--local-tone` sweep demonstrated the metrics preferring a visibly worse image,
  and a colour-path change was later scored as a regression on entropy at a median
  of four *millionths* of a bit. Treat a lopsided count at negligible magnitude as
  the noise it is — but check the magnitude before saying so.
- Colour level, hue and key placement are taste. Matching the camera is the right
  answer in ordinary conditions and the wrong one at night and extreme ISO.
- **A metric can be structurally unable to reward the thing you are testing.**
  `crushed_fraction` could never favour the owned colour path, because the path it
  was being compared against produces no negatives to crush. That was decided
  before a frame was rendered. When a change loses, ask whether the axis *could*
  have shown it winning.

### Freeze the controller while the layers under it move

No new exposure heuristics while the colour core is in flight, because a constant
tuned against a signal that is about to change is a constant that will need
retuning. Every sidecar records `controller_version`, so a later comparison against
a `v2` is mechanical rather than archaeological — the corpus grades already on disk
say which controller produced them. **Keep the grades.**

## What is next

Ordered by what each one unblocks, not by release.

### Completed in the current implementation

Hot/dead CFA-site suppression, clipped-highlight reconstruction (partial and,
since the A7C sparkle-highlight corpus below, fully clipped too), adaptive
PPG/RCD/AMaZE-class Bayer selection, and the DNG 1.7 matrix model are
implemented, deterministic, reported, and included in the versioned automatic
profile. The owned interpolators are guarded behind low-noise/low-alias tests
because dense-branch validation still favors mature PPG. The DNG path covers
one/two/three illuminants, custom illuminant data, three/four camera channels
and ReductionMatrix.

The deliberately-clipped corpus class arrived as `raw/arw/_DSC0922.ARW` and
`_DSC0924.ARW`: sun glints on rippling water and a lens-flare core, all three
raw channels clipped at once. Reconstruction had no surviving channel to
anchor on there and left the as-shot white-balance spread (A7C red ~2.3x, blue
~1.6x against green) exposed as a magenta cast — see `CHANGELOG.md`. Fixed by
falling back to the clipped channel with the largest white-balance coefficient
as the anchor when none survive.

The next requirement for these stages is corpus evidence: fine repeating
detail, high ISO and actual four-channel/triple-illuminant profiles should be
added as they become available.

### 1. Scene-class policies

High-key, low-key, night, backlit, flat — driven by the statistics split this
program already computes. Night first: it is the documented failure and the preview
oracle already half-solves it. **Each policy lands only with its corpus class as
regression evidence**, which is what makes this wait on the corpus rather than on
cleverness.

### 2. The grading session

The acceptance test for criterion 3, and the thing that ultimately decides whether
this program works: a blind side-by-side pass over the paired corpus, every frame
graded better / equal / worse / **ruined**. Target zero ruined, with "worse"
confined to classes that have a documented follow-up.

This is also how the learned controller eventually becomes plausible — graded
corpus passes *are* the training data. Which is why the grades get kept.

## What is deliberately not being done

- **Semantic analysis and ONNX** — faces, sky, subject detection. Waits until the
  non-neural controller's failures are *classified* on the corpus. The person and
  backlit corpus classes are being gathered now precisely so that classification is
  possible later.
- **A user-tunable policy file.** It serves users who want to adjust, which is the
  workflow this program exists to avoid. `--preset` is enough.
- **Candidate rendering with automatic selection.** Planned, then rejected on
  review: on the three scene classes where the controller is weakest (night,
  extreme ISO, deep shadow) the available metrics carry the *wrong sign*, so a
  selector would systematically pick the frame a human rejects. Revisit only after
  scene-class policies and metrics with real resolution on the contested axes. See
  `docs/candidate-render-plans-0115.md`.
- **A general lens-correction database.** Standard DNG
  `WarpRectilinear`/`FixVignetteRadial` opcodes are now applied when present;
  proprietary RAWs without portable coefficients remain intentionally
  uncorrected.
- GUI, camera looks/DCP, X-Trans quality, EXR export, GPU.

## Standing lessons

Generalisable, each bought with a wrong turn:

1. **Removing a clip can be worse than keeping it**, if a downstream stage was
   relying on the clip's guarantee. Look for the stage that depended on the
   discarded property — and check the *sign* of any signed sum it takes.
2. **A one-line inconsistency beats a clever fix.** The crushed-shadow regression
   was two clamps in the same function disagreeing about where black is. The
   elaborate operator proposed to fix it was, on inspection, mathematically
   impossible.
3. **When numbers look wrong, suspect the instrument first.** A hue comparison
   silently declined a third of the corpus because two aspect ratios agreeing to
   0.13% rounded to grids one pixel apart. Separately, `crushed_fraction` and
   `clipped_fraction` were measured on our side in 16 bits and on the camera's in
   8, so the same picture scored differently depending on which side of the
   comparison it was on.
4. **A metric can describe the fix and call it a regression.** Preserving sub-black
   samples removed a magenta cast from every shadow — plainly visible, mechanically
   explained — while making `crushed_fraction`, `mean_level`, entropy *and*
   gradient all worse. Three of the four scorecard axes voted against an
   unambiguous improvement, because what turned black was noise that used to turn
   into coloured haze. No axis on the scorecard could see the cast at all. This is
   the concrete reason the grading session cannot be replaced by a scorecard.
5. **State the measurement that justifies each change, in the code**, next to the
   constant it justifies. Every tuned value here has one, and the ones that turned
   out to be compensations for a bug elsewhere were found by re-reading those
   comments.
6. **Verify before generalising.** An external review that was largely correct
   still had its emphasis backwards on which half of a defect mattered, and a
   confidently-asserted licence bug did not exist. Check the source.
