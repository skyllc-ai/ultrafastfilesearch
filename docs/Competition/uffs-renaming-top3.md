# NTFS MFT File Search Tool — Naming Deep‑Dive + Top 3 Name/Command Combos

> **Deliverable format:** raw Markdown (per request)

## 1) What your tool is (branding-relevant summary)

From your README, your differentiators are unusually clear:

- **Core trick:** read the NTFS **Master File Table (MFT)** “like a phonebook” instead of recursive directory walking, then query the in-memory dataset.
- **Query engine:** use a **columnar/DataFrame approach (Polars)** for fast filtering/sorting over millions of file records.
- **Workflow modes:** fast ad-hoc search **and** the ability to build **Parquet indexes** for repeatable instant queries.
- **Speed claim:** you’re competing with “Everything / WizFile”-style tools and positioning as the fastest.
- **Distribution shape:** CLI-first with additional TUI/GUI binaries; subcommands include: `search`, `index`, `info`, `stats`, `save-raw`, `load-raw`.

These points drive naming:
- Your value is not just “fast search”; it’s **fast search because you treat the filesystem as a table**.
- The name should feel like an **engine**, not a one-off script.

---

## 2) Why your current name is hard to protect

“Ultra Fast File Search / UFFS” has the classic problem: it’s **descriptive** and therefore weak as IP.

- Descriptive names often require proof of “secondary meaning” to be protectable, and you’ll still struggle to stop others from using similar phrases.
- Acronyms like UFFS also collide easily across unrelated domains (software, protocols, projects, etc.).

So the goal is to move up the trademark distinctiveness ladder:
**Generic → Descriptive → Suggestive → Arbitrary → Fanciful**  
You want *Suggestive/Arbitrary/Fanciful*.

---

## 3) Competitive naming patterns (what works in this niche)

Fast file-search brands typically use one of three patterns:

1) **Totality / immediacy**  
   - Example vibe: *Everything* (implies “all files, instantly”)

2) **Wizardry / cleverness**  
   - Example vibe: *WizFile* (implies “smart trick makes it fast”)

3) **Speed metaphors**  
   - Example vibe: *SwiftSearch* (speed adjective + “search”)

Your opportunity: lean into the *technical story* (MFT + table-query) **without** becoming purely descriptive.

---

## 4) CLI naming constraints (important and commonly overlooked)

Avoid commands that are likely to collide with:
- Built-ins like `find`, `where`, `dir`
- Popular cross-platform tools: `fd`, `rg`, `locate`
- Overloaded abbreviations: `fs` is too generic and unprotectable as a brand.

A good command name is:
- 5–9 characters
- 1 word
- easy to pronounce
- unique enough to Google

---

## 5) Candidate names rejected (due diligence notes)

During preliminary clearance-style checks, a few tempting candidates were rejected:

- **Oxidex / OxiDex** — already used in software and also appears in trademark records (high collision risk).
- **VoluDex** — already used by an unrelated product/brand (undesirable association risk).
- **PathJet** — already appears as an existing company/identity (collision risk).

This is why the final list below avoids those stems.

---

# 6) Top 3 protectable, catchy Name + Command combinations

## 1) **MFTly** — command: `mftly`

**Mnemonic:** “MFT + fly” → fast NTFS search.

**Why it’s catchy**
- Short, pronounceable, and evokes speed immediately.
- Technical users instantly “get it” (MFT), non-technical users still read “fly”.

**Why it’s protectable**
- It’s a coined composite (not a generic phrase like “fast file search”).
- Even though “MFT” is descriptive, the *full mark* “MFTly” is more distinctive and defendable than “MFT Search”.

**Command ergonomics**
- `mftly search foo`
- `mftly index --output idx.parquet`
- `mftly stats --drive C:`

**Sub-brand/binary family**
- `mftly` (CLI)
- `mftly-tui`
- `mftly-gui`
- `mftly-mft` (low-level / debugging)

**Risk note**
- “MFT” can also mean “Managed File Transfer” in enterprise contexts. You can neutralize confusion by consistently writing **“MFT (Master File Table)”** in your tagline and docs.

**Tagline option**
- “Fly through NTFS.”

---

## 2) **MFTBolt** — command: `mftbolt`

**Mnemonic:** lightning speed + technical credibility.

**Why it’s catchy**
- “Bolt” is a universal speed metaphor and feels powerful.
- Still clearly tied to the MFT mechanism, so it doesn’t sound like a generic grep clone.

**Why it’s protectable**
- “Bolt” alone is weak; **MFTBolt** as a combined coined mark is much more distinctive in your niche.

**Command ergonomics**
- Very script-friendly: `mftbolt search "*.jpg" --drive D:`
- Clear when pasted in docs: it doesn’t look like a shell built-in.

**Sub-brand/binary family**
- `mftbolt`, `mftbolt-tui`, `mftbolt-gui`

**Tagline option**
- “Bolt-fast NTFS search.”

---

## 3) **ParqPulse** — command: `parqpulse`

**Mnemonic:** *Parq* (evokes Parquet/indexing) + *Pulse* (instant query loop).

**Why it’s catchy**
- “Pulse” suggests repeated, immediate results (“type → results”), matching the UX of fast search tools.
- “Parq” makes the name distinctive (and avoids the generic word “parquet”).

**Why it’s protectable**
- More coined/suggestive than the usual “FastSearch” names.
- Works well if you want to position this as an “engine” for search and indexing.

**Command ergonomics**
- Slightly longer than the first two, but still fine for daily CLI use.
- Great for marketing copy: it sounds like a product.

**Sub-brand/binary family**
- `parqpulse`, `parqpulse-tui`, `parqpulse-gui`

**Tagline option**
- “Pulse your disks. Query instantly.”

---

## 7) Which one should you pick?

If your audience is primarily power users / sysadmins / developers and you want the technical story front-and-center:
- **Pick MFTly / `mftly`**.

If you want the most “obviously fast” name with strong CLI feel:
- **Pick MFTBolt / `mftbolt`**.

If you want to emphasize the index/query architecture and future “engine/platform” story:
- **Pick ParqPulse / `parqpulse`**.

---

## 8) Final “do this before you ship the rename” checklist

Not legal advice — just practical steps:

1) **Knockout searches**
   - `"NAME" software`
   - `"NAME" command line`
   - `"NAME" GitHub`
   - `"NAME" crates.io`

2) **Trademark searches** (where you’ll sell/distribute)
   - US (USPTO), EU (EUIPO), UK (UKIPO), etc.
   - Focus on software / downloadable apps / developer tooling classes.

3) **Ecosystem availability**
   - GitHub org/user name
   - crate name (crates.io)
   - package managers (winget/scoop/choco) identifiers

4) **Migration ergonomics**
   - Keep a compatibility shim:
     - old `uffs` binary prints a deprecation notice and forwards to the new binary for 1–2 releases.

---

## 9) Minimal migration suggestion (so you don’t break users)

- vNext: ship both:
  - `uffs` → wrapper that calls new command
  - `mftly` (or the chosen new name) → real binary
- vNext+1: docs default to new command, wrapper remains
- vNext+2: remove wrapper (or keep indefinitely if tiny)

---

## Sources consulted (URLs in code blocks for portability)

```
WizFile official “reads the MFT directly” explanation:
https://antibody-software.com/wizfile/about

SwiftSearch description (reads file table directly / NTFS):
https://sourceforge.net/projects/swiftsearch/

Everything (voidtools forum) statements about creating DB from NTFS MFT:
https://www.voidtools.com/forum/viewtopic.php?p=60486

Your current project README (internal):
<local file: README.md>
```
