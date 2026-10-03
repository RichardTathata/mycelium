## [2026-10-03] ingest | the guarantee catalogue — I2's deferred golden, and Q1 closed

**What:** `guarantee::catalogue_markdown` / `catalogue_json`, the gate test under `compliance,a2a`,
`docs/reference/guarantee-catalogue.{md,json}`; `mycelium --help` names the profiles.

**Durable knowledge:** a state matrix is build-dependent by nature — `not_in_build` is a fact about the
binary — so the golden is generated and checked under one named feature set (CI's), and the document
says which. The reasoning that keeps the rule catalogue honest applies: a generated document cannot prove
a description true, the gate proves it current; the matrix proves what the resolvers *say* under those
configurations, which is exactly what the startup report would log.
