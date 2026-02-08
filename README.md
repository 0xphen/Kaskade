# ⚡ Kaskade

**Conditional Micro-Swap Execution Engine for TON DeFi**

**Kaskade** is a **Rust-based, non-custodial execution engine** for the **TON blockchain** that performs **condition-based micro-swaps**.

Instead of executing swaps on a fixed schedule, Kaskade executes **only when market conditions are favorable** — based on real-time liquidity, spread, trend, and slippage signals.

At its core, Kaskade introduces a **pulse-driven execution primitive** for TON DeFi.

---

## Why Kaskade?

Most TON DeFi users execute swaps during sub-optimal conditions:

* Spread widens → worse price
* Slippage spikes → lower output
* Local downtrend → buying into weakness
* Thin liquidity → high price impact

Existing tools (wallets, bots, DCA strategies) are **time-based**, not **market-condition-based**.

**Kaskade executes trades when the market itself signals “now is a good moment.”**

---

## Pulse-Based Execution Model

Kaskade continuously ingests market data and computes **pulses**.

A **pulse** is a short-lived market signal indicating favorable execution conditions.

> “A pulse means the market is currently safe and efficient to trade.”

Each user defines constraints (spread, trend, slippage), and Kaskade executes micro-swaps only when all constraints are satisfied.

---

## Pulse Types

| Pulse          | Purpose                                |
| -------------- | -------------------------------------- |
| **Spread**     | Avoids wide bid–ask spreads            |
| **Slippage**   | Ensures acceptable execution output    |
| **Trend**      | Avoids buying into local downturns     |
| **Depth**      | Ensures sufficient liquidity           |
| **Time-Decay** | Safety fallback to guarantee progress  |
| **Volatility** | Avoids chaotic market conditions       |
| **Imbalance**  | Detects directional liquidity pressure |

---

## High-Level Architecture

Kaskade is composed of four coordinated subsystems:

### 1. `market/`

Market data ingestion and signal computation.

* Polls **STON.fi** pool state
* Consumes **Omniston RFQ** data
* Maintains rolling windows
* Computes spread, trend, and depth pulses
* Normalizes metrics for scheduling

### 2. `scheduler/`

Decides *when* to execute.

* Applies pulse-based eligibility filters
* Fair selection (round-robin / DRR)
* Cooldowns and per-tick caps
* Emits execution intents

### 3. `executor/`

Turns decisions into real swaps.

* Re-validates market conditions
* Builds swaps using **Omniston gRPC**
* Produces TonConnect payloads
* Tracks execution outcomes

### 4. `emc/` — Execution Manager Contract

A non-custodial on-chain contract.

* Holds user-approved tokens
* Executes swaps directly against STON.fi
* Enforces user constraints on-chain
* Allows withdrawal at any time

**All swaps are executed by the contract — never by backend-signed transactions.**

---

## Non-Custodial Execution (EMC)

The **Execution Manager Contract (EMC)** enables fully automated execution without custody.

### Setup (one-time)

1. User deploys EMC via TonConnect
2. User deposits Jettons + TON for gas
3. Execution parameters are locked on-chain

### Automated Execution

* Backend detects pulses
* Executor sends authorized messages
* EMC executes swaps against STON.fi
* Output tokens are returned to the user

### Withdrawal

* User can withdraw remaining funds anytime

---

## STON.fi & Omniston Integration

### Omniston

* RFQ WebSocket
* Route-aware pricing
* gRPC `buildTransfer`
* Multi-hop routing

### STON.fi

* Pool liquidity polling
* Swap simulation
* DEX-level execution via EMC

---

# 🧪 Development & Local Setup

## Prerequisites

* Rust (stable)
* Cargo
* SQLite
* SQLx CLI

Install SQLx CLI:

```bash
cargo install sqlx-cli --no-default-features --features sqlite
```

---

## Clone & Build

```bash
git clone https://github.com/your-org/kaskade.git
cd kaskade
cargo build
```

---

## Database Setup (SQLite + SQLx)

### 1️⃣ Set `DATABASE_URL`

From the **repo root**:

```bash
export DATABASE_URL="sqlite://$(pwd)/backend/dev/kaskade_dev.db"
```

Verify:

```bash
echo $DATABASE_URL
```

---

### 2️⃣ Create DB directory and file

```bash
mkdir -p backend/dev
touch backend/dev/kaskade_dev.db
```

---

### 3️⃣ Run migrations

```bash
cargo sqlx migrate run
```

Verify tables:

```bash
sqlite3 backend/dev/kaskade_dev.db ".tables"
```

---

### 4️⃣ Seed dev data (optional)

If you have seed migrations, they will run automatically.
To inspect:

```bash
sqlite3 backend/dev/kaskade_dev.db "SELECT * FROM sessions LIMIT 5;"
```

---

## Running the Backend

```bash
cargo run -p backend
```

You should see logs indicating:

* Market pollers started
* Scheduler ticks
* Market metrics updates

---

## Running Tests

```bash
cargo test
```

For backend only:

```bash
cargo test -p backend
```

---
