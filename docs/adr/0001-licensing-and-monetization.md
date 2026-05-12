# ADR 0001 — Licensing and monetization model

- **Status**: Accepted
- **Date**: 2026-05-13
- **Decider**: Adrien Syrotnik (sole author at decision time)
- **Supersedes**: none

## Context

`claude-overlay` was started as a personal Dynamic-Island-style overlay for Claude Code hook events on Windows 11. It is currently licensed **MIT** with a single contributor.

The project has reached a point where it works end-to-end (notifications, AskUserQuestion routing, VS Code focus integration). Before publishing or actively distributing, we need to decide:

1. Which license future versions ship under.
2. Whether the project is monetized, and if so, how.

The constraint is that the target audience is niche (Claude Code power users on Windows), the author is a solo maintainer, and the maintenance overhead of complex commercial licensing schemes (BSL, dual-license, commercial license tracking) is not justified at current scale.

Options that were considered:

- **MIT alone** — fully permissive. Anyone can fork, modify, redistribute, sell. No mechanism to capture any revenue from third-party use.
- **BSL (Business Source License)** — source-available; commercial use restricted for N years, then auto-converts to a permissive license. Prevents competitors from re-hosting/reselling. Adds explanatory burden ("is this open source?" → "no, source-available"). More overhead than warranted at this scale.
- **MIT + Commons Clause** — adds a "no sale" addendum. Same family of issues as BSL with less legal precedent.
- **Proprietary** — maximum control but kills community contribution; no upside at current adoption.
- **AGPL + dual commercial license** — heavy machinery aimed at B2B; mismatched with a personal-developer tool.
- **MIT + paid prebuilt binary** — keep the code fully open, sell the convenience of a signed, auto-updating installer.

Examples of the last pattern shipping today: Sublime Text (proprietary variant), Tiled (open-source + itch.io paid binary), many indie itch.io tools and games, TablePlus, etc.

## Decision

1. **License remains MIT** for all source code, indefinitely. We do not switch to BSL, Commons Clause, AGPL, or a proprietary license.
2. **Monetization is a paid prebuilt binary** distributed via a third-party platform (likely Gumroad or Lemon Squeezy — to be finalized at v0.1 launch). Initial target price: **€10 HT** (one-time, perpetual use, includes updates within the same major version). May be adjusted at launch based on market feedback; this ADR is not a price commitment.
3. **Source build remains fully supported and documented**: anyone can clone the repo, build the binary themselves, and use it freely under MIT. No artificial friction is added to the source build (no code obfuscation, no key checks, no telemetry gating).
4. **The "moat" is convenience**, not legal protection: signed installer, one-click setup, auto-updates, optional support. Users who pay are paying for time saved and for sponsoring continued development — not for access to the software.
5. **Donations are accepted** but framed as supplementary, not as the primary funding path.

## Why this and not BSL / commercial licensing

The honest assessment is that at the project's current size, the dominant risk is **lack of distribution**, not **competitors forking and reselling**. BSL solves a problem we don't have yet, at the cost of explanatory friction with potential users and contributors. If/when the project sees real traction (e.g., 50+ paying users, B2B inbound requests for team features), a future ADR can revisit the license — for example, splitting the codebase into an MIT core + a proprietary/closed "team" or "cloud" module, which is a cleaner separation than relicensing the whole project.

Crucially, **any commit already published under MIT remains MIT forever**. A future license change only binds new contributions. Choosing to relicense the project later is therefore always possible but only protects forward — there is no retroactive lock-in we incur by waiting.

## Consequences

**Positive**

- Zero legal overhead. No license tracking, no commercial agreements to write, no compliance burden on enterprise users.
- Community contribution stays maximally easy (PRs, forks, derivative works are all fine).
- Pricing flexibility: we can change the price, run promos, give the binary away to OSS contributors, bundle support tiers, etc., without touching the license.
- Trust signal: source is fully open, so users (especially security-conscious devs) can audit before installing.

**Negative**

- A competitor can fork the codebase, build their own signed binary, and undercut on price (or give it away). Mitigation: brand recognition, being the canonical source, shipping faster than a fork can keep up with.
- Revenue is uncapped on the upside but also low-floor — niche audience means realistic Year 1 revenue is likely a few hundred to low thousands of euros at most. This is acceptable; the project does not need to be the author's primary income.
- Some users will rebuild from source rather than pay. That is by design and not a failure mode — they are still adopters and may contribute back.

**Out of scope / deferred to a future ADR**

- Pricing tiers beyond the single €10 HT one-time offer (team licenses, education discounts, lifetime vs. yearly-update splits).
- Telemetry or update channels in the paid binary.
- Choice of payment platform (Gumroad vs. Lemon Squeezy vs. Polar vs. self-hosted Stripe Checkout).
- Any decision related to a hosted/SaaS team-sync product — that would require its own ADR with a different licensing analysis.

## Links

- License file: [LICENSE](../../LICENSE) (MIT)
- Project state and scope: see `docs/2026-04-24-claude-overlay-design.md`
