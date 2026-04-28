# Design Philosophy

*This document is the soul of the project. It predates and supersedes all architectural
decisions. When a technical choice feels uncertain, return here.*

---

## What This Software Is

A professional-grade, single-site NEXRAD Level II radar analysis application for Linux.
Built for people who use radar seriously — storm chasers, emergency managers, meteorologists,
and trained spotters — during conditions when the software must not fail them. It is a
focused instrument for a specific, important task, and that focus is its greatest strength.

---

## Core Principles

### 1. The Instrument Principle

The user is looking *through* the software at the atmosphere, not *at* the software itself.
Every design decision must serve the data, not the interface. Chrome exists only to the
extent it enables access to data. Everything else is noise.

### 2. Stability Is a Trust Relationship

This software may be running during a tornado warning. A crash in that moment is not an
inconvenience — it is a failure with real consequences. Stability is therefore an ethical
obligation, not a quality attribute. It governs error handling, dependency selection,
testing discipline, and how we treat edge cases in data. Trust is built slowly and
destroyed instantly.

### 3. Lightweight by Design, Not by Accident

A user must be able to run multiple simultaneous instances — each pointed at a different
radar site — without meaningful resource contention. This is a core use case, not a stretch
goal. Fast startup, small memory footprint, and non-blocking render paths are constraints
that must be honored at every layer of the architecture.

### 4. Clean, Uncomplex Code

Complexity is a cost. The correct solution is the simplest one that fully solves the
problem. Code should be readable by someone unfamiliar with the codebase. Dependencies are
maintenance surface and are chosen conservatively. Modules have clear, narrow
responsibilities. This discipline is what makes the software maintainable and extensible
over time.

### 5. Basic Is a Feature

Restraint is intentional. This software does one thing — single-site NEXRAD Level II
analysis — and does it exceptionally well. The minimal basemap is a deliberate choice, not
a limitation. Feature requests that broaden scope beyond the core mission are evaluated
skeptically. The question is never "could we add this?" but "does this serve the instrument
principle without compromising anything else?"

### 6. Respect for the User's Workflow

This software is built for people who know what they are doing. It is keyboard-friendly,
spatially stable, and supports the real workflow of serious radar analysis: multiple windows,
rapid product switching, fast pan and zoom. We design for how experienced operators
actually work, not for an imagined user who needs to be guided through every interaction.

### 7. Security as a First-Class Concern

This software is intended for use in government, corporate, and defense environments, not
only at home. It must be approvable by IT and security administrators in those contexts.
This means minimal dependencies with auditable security posture, memory-safe implementation
by construction, no undisclosed network connections, no telemetry, no elevated privileges,
reproducible builds, and clear documentation of all data handling. The only network
connections made are those the user explicitly initiates — radar data and map tiles, nothing
else. Security is not added at the end. It is a constraint honored from the beginning.

### 8. Open Source by Conviction, Not by Default

This software is open source because transparency is the asset. A public codebase can be
audited by security reviewers, evaluated by government procurement officers, studied by
the meteorological community, and contributed to by domain experts. Closed source would
foreclose all of that. The goal is not to give the software away — it is to build a
visible, verifiable body of work that establishes credibility in a domain where that
credibility has real career and commercial value. The code is the credential.

---

## What We Are Building Against

- Not a web application in a desktop shell. Native compiled code only.
- Not a platform or plugin ecosystem. A tool.
- Not a feature showcase. The sophistication should be invisible.
- Not feature-complete on day one. Fewer things done with full integrity beats many things done poorly.

---

## The Standard

When a decision is uncertain: *would a serious radar operator, in the middle of an active
severe weather event, find this software reliable, fast, and transparent?*

If yes, proceed. If uncertain, simplify. If no, start over.
