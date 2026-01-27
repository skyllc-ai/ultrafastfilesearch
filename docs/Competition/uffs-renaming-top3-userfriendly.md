# UFFS rename — user‑friendly, brandable options (Top 3)

> **Context (from your README):** a Windows file search/index tool that’s *unusually fast* because it reads NTFS metadata directly once, then queries in-memory and/or from a persisted Parquet index. It ships as CLI-first with optional TUI/GUI, and you want a name that **normal users can grok** (no “MFT” in the brand), while still being **distinctive enough to protect**.

> **Not legal advice.** This is “practical brand + knockout‑search” guidance. Final clearance requires proper trademark searches (USPTO/EUIPO/UKIPO/etc.) and an attorney if you’re filing in multiple jurisdictions.

---

## What makes a name *protectable* in this space

**Avoid** names that are purely descriptive:  
- “Ultra Fast File Search”, “Instant File Search”, “Fast File Finder”, etc.  
These are hard to enforce because competitors can use similar phrases.

**Prefer** names that are:
- **Suggestive** (hint at the benefit without describing the mechanism)
- **Arbitrary** (real word, unrelated to file search)
- **Fanciful** (made-up word)

CLI command names can be more functional, but if you want **a brand + a command** that travels well, treat the command as part of the product identity.

---

## Shortlist constraints (what I optimized for)

- **Relatable to normal users**: “Find/Seek/Search” implied without acronyms like MFT/NTFS/Parquet.
- **Distinctiveness**: avoids generic “Fast/Quick/Ultra/Turbo/Swift” constructions.
- **CLI ergonomics**: 5–9 characters, easy to type, unlikely to collide with built-in Windows commands (`dir`, `where`, `find`, etc.).
- **Product line extensibility**: easy to form `name-tui`, `name-gui`, `name-mft` without sounding weird.

---

# Top 3 name + command combinations

## 1) **OxideFind** — command: `oxfind`

**What it communicates (average user):**
- “Find” is obvious.
- “Oxide” feels *tough/industrial* and hints at “Rust-powered” without being inside-baseball.

**Why it’s brandable/protectable:**
- “OxideFind” is a **coined composite**. It’s not a generic phrase for file searching.
- It also supports a story: **Rust (oxide) + find**.

**CLI feel:**
```bash
oxfind "project*"
oxfind "*.rs" --drives C,D
oxfind index --drive C --output c.parquet
oxfind search "*.rs" --index c.parquet
```

**Naming family:**
- `oxfind` (CLI)
- `oxfind-tui`
- `oxfind-gui`
- `oxfind-core`

**Tagline options:**
- “Find files instantly.”
- “Rust-powered instant file search.”

**Risk notes / diligence:**
- “Oxide” is used in other software contexts, so you still need a clearance search for **OxideFind** as a whole mark.
- Bonus: the domain `oxidefind.com` appears to be available for acquisition (at least at the time of the knockout check), which helps brand cohesion.

---

## 2) **NeedleStack** — command: `nstack`

**What it communicates (average user):**
- Everyone understands “needle in a haystack.” This says: **you find the needle**.
- “Stack” implies a pile of stuff (files) you can sift quickly.

**Why it’s brandable/protectable:**
- It’s **suggestive** rather than descriptive. You’re not naming the feature (“file search”), you’re naming the experience (“find the needle”).

**CLI feel:**
```bash
nstack "*.pdf" --drive C
nstack "tax*" --min-size 1MB --format json
nstack index --drives C,D --output all.parquet
nstack search "project*" --index all.parquet
```

**Naming family:**
- `nstack` (CLI command)
- `needlestack` (marketing name, repo name, website)
- `nstack-tui`, `nstack-gui`

**Tagline options:**
- “Find the needle. Instantly.”
- “Search millions of files in a blink.”

**Risk notes / diligence:**
- “NeedleStack” is uncommon enough to be *potentially* strong, but you must check for existing software marks (especially developer tools).

---

## 3) **PathPulse** — command: `ppath` *(or)* `pathpulse`

**What it communicates (average user):**
- “Path” = files/folders (intuitive)
- “Pulse” = instant response, “type → results” rhythm

**Why it’s brandable/protectable:**
- It’s **suggestive**. It doesn’t describe “file search” directly, but clearly lives in that world.

**CLI feel (two good command strategies):**

**A) Keep brand == command (simple):**
```bash
pathpulse "*.jpg" --drives C,D
pathpulse index --drive C --output c.parquet
pathpulse search "*.jpg" --index c.parquet
```

**B) Short command + branded display name (more “mnemonic”):**
```bash
ppath "*.jpg" --drives C,D
ppath index --drive C --output c.parquet
```

**Naming family:**
- `pathpulse` (brand + repo)
- `ppath` (short CLI alias)
- `pathpulse-tui`, `pathpulse-gui`

**Tagline options:**
- “Pulse through your files.”
- “Instant results across every drive.”

**Risk notes / diligence:**
- Compound common words can be trademarked when used as a whole, but enforcement can be weaker than a more fanciful mark. Consider pairing with a distinctive logo + consistent styling.

---

## Recommendation (if you want one “best” pick)

If you want **one name that’s:**
- memorable,
- not technical,
- still obviously “search”,
- and tells the Rust story cleanly:

➡️ **OxideFind / `oxfind`**

If you want the most **consumer-relatable** mental model:

➡️ **NeedleStack / `nstack`**

If you want a name that feels **product-y** and scales to a GUI/TUI:

➡️ **PathPulse / `pathpulse`**

---

## Migration plan (don’t break existing users/scripts)

For 1–2 releases:

1. Keep `uffs` as a compatibility shim:
   - prints a one-line deprecation message to stderr
   - forwards args to the new binary

2. Update docs to the new name, but include:
   - “formerly UFFS” on the first page
   - install instructions that create both executables (or an alias)

---

## Minimal clearance checklist before shipping the rename

1. **Knockout web searches**  
   - `"NAME" file search`
   - `"NAME" CLI`
   - `"NAME" Windows`
   - `"NAME" GitHub`
   - `"NAME" crates.io`

2. **Trademark searches** (where you ship)  
   - USPTO (US), EUIPO (EU), UKIPO (UK), WIPO Global Brand DB  
   - Search both the **word mark** and confusingly-similar variants.

3. **Ecosystem availability**  
   - GitHub org/repo names  
   - crates.io crate name  
   - winget/scoop/chocolatey identifiers  
   - domain + social handles (optional but helpful)

---

## Optional: a clean “brand + descriptor” way to write it everywhere

- **OxideFind** — *instant file search for Windows*  
- **NeedleStack** — *instant file finder*  
- **PathPulse** — *instant drive search*

That keeps the mark distinctive while still making the function obvious to a first-time user.
