# Meridian / Node API — Landing Page Brief
_Written: 2026-03-07_

---

## Brand Clarification

The GitHub repo and folder are `meridian-api`. The live domain is `nodeapi.ai`. Before building the landing page, decide which name wins. **Node API** is more search-friendly and domain-generic. **Meridian** is more evocative and cartographically themed. Recommendation: **Meridian** as the product name, `nodeapi.ai` as the domain — similar to how Stripe lives at stripe.com not payments.com.

---

## The Two Audiences

### Audience 1: Humans (Developers & GIS Professionals)

**Who they are:**
- Developers building AI agents that need geospatial capabilities (route analysis, coordinate transforms, spatial queries, batch processing)
- GIS professionals who want to add AI/agent automation to their workflows
- Researchers who need programmatic GIS without standing up ArcGIS or QGIS infrastructure
- Hobbyists building location-aware apps who can't afford Esri licensing

**What they care about:**
- What operations does it actually do? (Be specific — not "geospatial processing", but "reproject shapefiles, calculate centroids, clip by polygon, batch transform coordinates")
- How much does it cost? Is it predictable?
- Can I try it without a wallet/credit card?
- Is it production-reliable? (uptime, rate limits, Hetzner infra)
- Where's the documentation?

**What they DON'T care about (yet):** MCP, Solana, agents. Lead with the value, reveal the mechanism later.

---

### Audience 2: AI Agents (via MCP)

**How agents discover APIs:**
Agents find tools primarily through:
1. **MCP directories** — mcpservers.org, glama.ai, mcp.so, Smithery, punkpeye/awesome-mcp-servers. Meridian is already listed on several.
2. **`list_tools()` endpoint** — when an agent connects to an MCP server, it calls this first. Tool names, descriptions, and parameter schemas are the entire interface. Description quality matters enormously.
3. **GitHub topics** — `mcp`, `mcp-server`, `gis`, `geospatial` (already set on Meridian's repo ✓)
4. **LLM training data** — over time, well-documented APIs get embedded in model weights. Clear documentation = organic discovery.

**What agents care about:**
- Well-named tools with precise descriptions (the LLM reads these to decide which tool to call)
- Predictable input/output schemas
- Low latency
- Payment that doesn't require human intervention — this is where Solana shines

**The Solana angle — why it matters for agents:**
This is genuinely novel. The emerging standard for agent-to-API payments is the **x402 protocol** (HTTP 402 Payment Required), and Solana is the preferred chain due to speed + low fees. Meridian is ahead of the curve here. A few others exist (apiforchads-mcp, some crypto data APIs) but GIS + Solana + MCP is essentially unoccupied territory. 

The pitch: *An agent can call Meridian, pay ~$0.001 per operation in USDC, and get a spatial result — no API key, no subscription, no human in the loop.* That's the future of agent infrastructure.

---

## Landing Page Structure

### Hero
**Headline:** `GIS processing for AI agents — and the humans who build them.`  
**Subhead:** `Meridian is a geospatial API with an MCP server built in. Batch operations, coordinate transforms, spatial analysis. Pay per call with Solana, or use the REST API directly.`  
**CTA (human):** `Read the docs →`  
**CTA (agent):** `MCP endpoint: nodeapi.ai/mcp` (small, technical, below the fold or in nav)

---

### Section 1: What It Does (Humans)
Concrete list of operations — reproject, clip, buffer, centroid, bbox, format convert, batch (up to 10). Keep it scannable. Show a real API request/response example.

### Section 2: Built for Agents
Explain MCP briefly (1-2 sentences). Show the `list_tools()` output or a tool schema. Emphasize: no API key required for Solana-paying agents. Show a diagram: Agent → MCP → Meridian → Result + USDC deduction.

### Section 3: Pricing
Two tracks:
- **REST API:** [pricing model — TBD, key-based or Solana]
- **MCP / Agent:** Pay per call via Solana USDC. ~$X per operation. No subscription.

### Section 4: Quick Start
Two tabs: `REST` and `MCP`. Copy-pasteable examples for each.

### Section 5: Infrastructure
Hetzner, FastAPI, rate limiting (60 req/min), batch endpoint (/v1/batch, up to 10 ops). Trust signals.

### Section 6: Listed On
Logos/links: mcpservers.org, glama.ai, mcp.so, modelcontextprotocol/servers. Social proof for the agent ecosystem.

---

## Key Questions to Resolve Before Building

1. **Brand:** Meridian or Node API? (or Node API by Meridian?)
2. **Pricing:** What's the per-call Solana price? Is the REST API also key-based or Solana-only?
3. **Operations list:** What are all the current endpoints? Need full list for the "what it does" section.
4. **MCP tools:** What are the tool names/descriptions in the current MCP server?
5. **Demo:** Is there a live demo or playground? Even a simple curl example would help.

---

## Copy Directions to Explore

- **Technical but accessible** — not dumbed down, but not impenetrable. Target: a developer who knows Python but hasn't used GIS much.
- **Agents as first-class citizens** — most GIS APIs treat agents as an afterthought. Lean into being designed for them.
- **The "no API key" angle** — it's genuinely radical. "Your agent can call this API without you ever logging in." That's a hook.
- **Cartographic identity** — "Meridian" is a good name. Lean into the map metaphor. The logo already does this.

---

_Brief by Malko. Ready for Eian's review in the morning._
