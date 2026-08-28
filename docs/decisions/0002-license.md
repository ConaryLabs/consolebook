# ADR 0002: AGPL-3.0-only license

- **Status:** Accepted
- **Date:** 2026-08-27

## Context

Consolebook is intended to remain inspectable, self-hostable, and portable. A permissive license would allow a vendor to modify the application, operate the modified version as a network service, and withhold those modifications from the people using it.

Leaving a public repository unlicensed reserves all rights by default and prevents ordinary use, redistribution, and contribution. Delaying the choice also makes later relicensing dependent on permission from outside contributors.

## Decision

Consolebook is licensed under the GNU Affero General Public License version 3.0 only, using the SPDX identifier `AGPL-3.0-only`.

Contributions are accepted under the same license without a separate contributor license agreement or a broad relicensing grant.

The product will expose its license, running version, and source location in its interface. Operators who modify Consolebook and let users interact with it over a network are responsible for offering those users the Corresponding Source for that running modified version.

## Consequences

### Positive

- redistributed versions remain under the same software-freedom terms;
- users of a modified network deployment can obtain its source;
- a hosted vendor cannot keep service-only modifications proprietary; and
- contribution terms are clear before outside code arrives.

### Costs

- some procurement and legal teams prohibit or avoid AGPL software;
- license compatibility must be evaluated before combining Consolebook with other components;
- deployers of modified network versions must maintain a compliant source offer; and
- future relicensing requires consent from copyright holders unless rights are obtained separately.

## Rejected alternatives

### Apache-2.0

Rejected because its procurement familiarity does not offset the project's concern about proprietary hosted forks.

### AGPL-3.0-or-later

Rejected because the project is not pre-authorizing unknown future license versions. A later version can be adopted through an explicit relicensing decision with the required contributor consent.

### No license yet

Rejected because a public, all-rights-reserved repository invites confusion and makes outside contribution needlessly risky.
