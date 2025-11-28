# ⚡ Kaskade — Conditional Micro-Swap Execution Engine for TON

<div align="center">
  <img src="https://github.com/user-attachments/assets/placeholder" alt="STONIC Pulse Diagram" width="350" height="350"/>
</div>

**STONIC PULSE** is a **Rust-powered, non-custodial smart execution engine** for the **TON Blockchain**, built directly on top of the **STON.fi DEX API**, **Omniston RFQ WebSocket**, and **Omniston gRPC trade builder**.

It introduces the first **pulse-based execution primitive** on TON — allowing users to split large swaps into **multiple micro-swaps executed only when market conditions are favorable**, achieving better pricing, lower slippage, and more efficient routing.

STONIC PULSE works as an event-driven alternative to basic “Swap Now” and naive DCA. It continuously listens to live market data and executes trades **only on good pulses**: tight spreads, improving slippage, and favorable micro-trends.

---

# ⭐ **Why STONIC PULSE Matters**

STONIC PULSE solves real problems for TON DeFi traders:

### 🔹 **1. Users typically execute swaps at bad moments**

Spread widens → they overpay
Slippage spikes → they receive less
Short-term dips → they buy at local bottoms

### 🔹 **2. Existing tools are static**

* “Swap Now” ignores real-time market structure
* DCA executes at fixed intervals, not based on conditions
* Telegram trading bots do not use RFQ depth or micro-trend signals

### 🔹 **3. PULSE introduces *dynamic, market-condition execution***

Instead of time-based trades, STONIC PULSE executes **event-based** swaps triggered by:

* lower spread
* lower slippage
* improving micro-trend (avoid buying into dumps)
* (future) rising liquidity depth
* (future) lower volatility
* (future) imbalance signals across RFQ sources

**The TON ecosystem has nothing like this today.**
STONIC PULSE creates a new foundational DeFi primitive.

---

# 🧠 **Core Concept: Market “Pulses”**

A **pulse** is a micro-signal that indicates *now is a good moment to execute a trade*.

STONIC PULSE listens to the Omniston RFQ stream and detects pulses in real-time.

## 🔵 Supported in Full Version (7 Pulses)

| Pulse                 | Description                                                    |
| --------------------- | -------------------------------------------------------------- |
| **Spread Pulse**      | Execute when spread is tight (best pricing).                   |
| **Slippage Pulse**    | Execute when estimated slippage is low using `/swap/simulate`. |
| **Micro-Trend Pulse** | Execute when short-term price movement improves.               |
| **Volatility Pulse**  | Avoid turbulent micro-moments.                                 |
| **Depth Pulse**       | Execute when liquidity depth increases.                        |
| **Imbalance Pulse**   | Execute when RFQ flows favor your direction.                   |
| **Time-Decay Pulse**  | Safety fallback—execute if conditions never appear.            |

---

# 🚀 **MVP (10-Day Release)**

The MVP is a **Rust CLI tool** that performs:

### ✔ Live Omniston RFQ streaming

### ✔ 3 Pulse primitives (Spread, Slippage, Micro-Trend)

### ✔ Conditional micro-swap execution strategy

### ✔ Chunk planner (e.g., 200 TON → 20×10 TON swaps)

### ✔ Omniston gRPC `buildTransfer` integration

### ✔ TonConnect-ready output for manual signing

This is enough to showcase the full concept and win the grant.

---

# ⚙️ **How It Works**

<div align="center">
  <img src="https://github.com/user-attachments/assets/placeholder2" alt="Pulse Execution Flow" width="550"/>
</div>

### 1. **Connect to Omniston WebSocket**

The engine streams `quoteUpdated` events for the chosen pair.

### 2. **Compute Market Signals**

Each quote is transformed into metrics:

* mid-price
* spread (bps)
* slippage (via STON.fi API)
* micro-trend (rolling window)

### 3. **Detect a Pulse**

A pulse fires when:

```
spread < threshold 
AND 
slippage < threshold 
AND 
trend is favorable
```

### 4. **Trigger Execution**

Each pulse triggers one micro-swap until the user’s full amount is exhausted.

### 5. **Build TON Transaction**

Uses Omniston gRPC:

```
buildTransfer -> TON cell messages -> TonConnect link
```

### 6. **User signs & broadcasts**

(No custody. No stored keys. No trust required.)

---

# 🛠️ **Quick Start (CLI Tool)**

### 1. Clone & Build

```bash
git clone https://github.com/0xphen/stonic-pulse.git
cd stonic-pulse

cargo build --release --bin stonic-pulse
export PATH=$PATH:$(pwd)/target/release
```

### 2. Verify Installation

```bash
stonic-pulse --help
```

---

# ⚡ **Example: Smart Pulse Swap (20 TON → STON in 2 TON chunks)**

```bash
stonic-pulse execute \
  --from TON \
  --to STON \
  --total 20 \
  --chunk 2 \
  --spread-threshold 0.35 \
  --slippage-threshold 0.40 \
  --enable-trend
```

**Sample Output (illustrative):**

```
🔄 Streaming RFQ quotes...
📈 mid=2.004 | spread=0.28% | trend=↑ | slippage=0.21%
⚡ Pulse detected! Executing chunk 1/10 (2 TON)

🧱 Building transfer via Omniston gRPC...
✓ Transfer ready!
TonConnect link:
ton://transfer/...base64...

📈 mid=2.003 | spread=0.17% | trend=↑ | slippage=0.15%
⚡ Pulse detected! Executing chunk 2/10 (2 TON)
...
```

---

# 🌐 **STONIC PULSE API Integrations**

STONIC PULSE deeply integrates with the entire STON.fi + Omniston ecosystem:

### 🔹 **Omniston WebSocket**

* RFQ quote stream
* real-time spreads & prices
* route discovery

### 🔹 **Omniston gRPC**

* `requestForQuote`
* `buildTransfer`
* `trackTrade`

### 🔹 **STON.fi REST API**

* `/v1/swap/simulate` → slippage
* `/v1/markets`, `/v1/pools`, `/v1/stats/pool` (future pulse support)

STONIC PULSE is essentially a *reference implementation* for complex STON.fi integrations.

---

# 🛣️ **Roadmap**

## **Phase 1 — Pulse Engine MVP (10 days)**

* CLI tool
* 3 pulse primitives
* micro-swap chunking
* gRPC buildTransfer integration
* TonConnect output

✔ Grant-ready MVP

---

## **Phase 2 — Telegram Bot + Semi-Automatic Execution**

* Pulse sessions controlled through Telegram
* User grants permission via TonConnect session
* Semi-automatic multi-swap execution
* Execution notifications
* Session summaries
* Slippage protection

✔ Real users start using the system

---

## **Phase 3 — Full Execution Platform**

* All 7 pulses supported
* Portfolio strategies (rotation, hedging)
* Backtesting + analytics
* Web dashboard
* SDK for other bots to use pulse execution
* Protocol-level integrations

✔ STONIC PULSE becomes a TON trading primitive

---

# 🤝 **Contributing**

STONIC PULSE welcomes contributions from the TON community:

* Open issues for bugs or feature requests
* Send pull requests for improvements
* Discuss architecture in the repo discussions

---

# 📄 **License**

STONIC PULSE is released under the **MIT License**.

---

If you want, I can also generate:

✅ a **CONTRIBUTING.md**
✅ a **project diagram** (ASCII or image-ready)
✅ a **grant proposal formatted version**
✅ the **logo** and **banner** for STONIC PULSE

Just tell me.
