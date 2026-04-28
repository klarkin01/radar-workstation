# ADR-0009: Release Under an Open Source License

## Status
Accepted

## Context
A decision was required on whether the application would be commercial (paid license,
closed source) or open source. GR2Analyst, the primary reference application, is
commercial at $250 per license. The Linux weather enthusiast market is smaller than the
Windows market and more culturally resistant to paid software. The primary goals of this
project include building verifiable domain credibility, enabling security audit by
government and defense evaluators, and positioning for future opportunities in radar
network modernization. Revenue from software sales was evaluated against these goals.

## Decision
The application is released as open source. License TBD (MIT, Apache-2.0, or dual
MIT/Apache-2.0 are the leading candidates — see open-questions.md).

## Consequences
- The codebase is publicly auditable by security reviewers, government procurement
  evaluators, and the meteorological community.
- Community contributions are possible — domain experts can improve algorithms,
  platform maintainers can fix distribution-specific issues.
- Grassroots adoption among NWS staff and the storm chasing community is more likely
  without a purchase barrier.
- Software sales revenue is foregone. This is accepted. The career and credentialing
  value of a visible, high-quality open source application in this domain is assessed
  as exceeding likely software sales revenue from the Linux market.
- The project is positioned as a portfolio artifact and professional credential,
  particularly with respect to future radar network modernization opportunities.
- Future monetization paths (support contracts, consulting, government contracting)
  are not foreclosed by open source release.
