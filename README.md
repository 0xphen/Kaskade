# ⚡ **Kaskade — Conditional Micro-Swap Execution Engine for TON DeFi**

**Kaskade** is a **Rust-based, non-custodial execution engine** for the **TON Blockchain** that performs **condition-based micro-swaps**.

It introduces the first **pulse-driven execution primitive** on TON — enabling users to split a swap into multiple micro-swaps that execute **only when market conditions are optimal**.

Kaskade is built on the core primitives of the STON.fi and Omniston stack — RFQ quotes, route simulation, and gRPC trade building — transforming them into a powerful Rust-based automation engine for TON DeFi.

# **Why Kaskade?**

TON DeFi users often execute swaps during highly unfavourable market conditions:

* Spread widens → bad price
* Slippage spikes → poor return
* Local downtrend → buying the top
* Depth thins → higher execution cost

Most current tools (bots, wallets, DCA) are **time-based**, not **market-condition-based**.

**Kaskade is the first TON engine that executes trades automatically when the market says “now is the optimal moment.”**


# **Pulse-Based Execution (Market Condition Detection)**

Kaskade continuously consumes Omniston RFQ quotes and STON.fi simulations to determine if **a pulse** has occurred — meaning execution is favorable.

A *pulse* is a market micro-signal signaling:

> “This is a good moment to execute a micro-swap.”

# 🧩 **Feature Overview**

| Feature                                                       | Status       |
| ------------------------------------------------------------- | ------------ |
| Omniston RFQ WebSocket Integration                            | ✔            |
| STON.fi Swap Simulation API                                   | ✔            |
| Omniston gRPC `buildTransfer`                                 | ✔            |
| Spread Pulse                                                  | ✔            |
| Slippage Pulse                                                | ☐            |
| Micro-Trend Pulse                                             | ☐            |
| Scheduler (eligibility + RR selection)                        | ✔            |
| Execution Engine & Worker Pool                                | ✔            |
| Session Manager (user strategies)                             | ✔            |
| Telegram Bot Command Flow                                     | ☐            |
| Execution Manager Contract (EMC) for non-custodial automation | ☐            |
| Depth Pulse                                                   | ☐            |
| Volatility Pulse                                              | ☐            |
| Imbalance Pulse                                               | ☐            |
| Backtesting Engine                                            | ☐            |
| Strategy Studio & Web Dashboard                               | ☐            |

# **High-Level Architecture**

Kaskade consists of 4 coordinated subsystems:

### **1. `market/`**

Streams and processes TON market data.

* Connects to **Omniston RFQ WebSocket**
* Maintains rolling windows for trend detection
* Computes spreads, mid-price, and volatility using Omniston RFQ data, STON.fi DEX liquidity insights, and Omniston gRPC route information.
* Normalizes metrics for scheduler

### **2. `scheduler/`**

Decides *when* to pull the trigger.

* Eligibility checks (spread, slippage, trend, time-Decay, market-imbalance, depth, volatility)
* Per-pair round-robin selection
* Cooldowns + rate limits
* Sends `ExecutionRequest` to executor

### **3. `executor/`**

Turns decisions into real swaps.

* Reloads session state
* Fetches latest metrics
* Builds swaps using **Omniston gRPC `buildTransfer`**
* Generates TonConnect/TX payload
* Updates session state
* Sends user notifications

### **4. `emc/` — Non-Custodial Execution Manager Contract**

An on-chain contract:

* Holds the user’s approved tokens
* Stores swap parameters and constraints
* Accepts backend-triggered micro-swap messages
* Performs the actual Jetton transfers to **STON.fi DEX**
* Returns output tokens to the user
* Allows withdrawal of unused tokens anytime

This contract allows **fully automated execution with zero user interaction after setup**.

# **Architecture Diagram**

This diagram reflects the **correct flow**:

```
                           ┌──────────────────────┐
                           │   User (Telegram)    │
                           │  Creates Pulse Plan  │
                           └──────────┬───────────┘
                                      │
                                      ▼
                         (1) TonConnect Deployment + Deposit
                                      │
                                      ▼
                    ┌───────────────────────────────────┐
                    │    Execution Manager Contract     │
                    │     (holds tokens non-custodially)│
                    └─────────────┬─────────────────────┘
                                  │ Pulse-trigger messages
                                  ▼
                   ┌───────────────────────────┐
                   │        EXECUTOR           │
                   │  • re-check conditions    │
                   │  • Omniston buildTransfer │
                   │  • send FX msg to EMC     │
                   └───────────┬───────────────┘
                               │
                               ▼
                    ┌───────────────────────────┐
                    │         SCHEDULER         │
                    │ • Eligibility filtering   │
                    │ • Round-robin selection   │
                    └──────────┬────────────────┘
                               │ metrics
                               ▼
                    ┌───────────────────────────┐
                    │         MARKET            │
                    │ • Omniston RFQ stream     │
                    │ • STON.fi /simulate       │
                    │ • Trend & spread calc     │
                    └───────────────────────────┘
```

**Actual swap happens inside the EMC**, not via backend-signed transactions.

# 🔒 **Non-Custodial Execution with EMC**

The **Execution Manager Contract (EMC)** makes Kaskade *fully automatic* while staying non-custodial.

### Setup (one-time TonConnect action)

1. User initiates session from Telegram/CLI
2. Kaskade backend provides TonConnect payload
3. User signs:

   * Contract deployment (StateInit)
   * Token deposit (Jetton transfer)
4. EMC is deployed and funded with TON for gas

### Automated Execution (hands-off)

* Kaskade reads market data
* Scheduler triggers pulses
* Executor sends authorized internal messages to EMC
* EMC executes micro-swaps directly with **STON.fi DEX**
* Output tokens are returned to the user’s wallet

### Withdrawal Anytime

* User sends a withdrawal message
* EMC returns remaining Jettons + unused TON

All logic is **non-custodial** and **verifiable on-chain**.

## 🛠️ STON.fi & Omniston Integrations

### **Omniston**
✔ RFQ WebSocket  
✔ Route-aware pricing  
✔ gRPC Trade Builder (`buildTransfer`)  
✔ Multi-hop routing  
✔ Aggregated liquidity depth  

### **STON.fi**
✔ `/v1/swap/simulate` for slippage  
✔ `/v1/pools` for depth analysis  
✔ `/v1/markets` for volatility inputs  
✔ DEX contract-level swap calls (via EMC)  

## 🗺️ Roadmap

### **Phase 1 — MVP (Complete)**  
✔ RFQ stream  
✔ Spread / Slippage / Trend pulses  
✔ Scheduler  
✔ Executor  
✔ gRPC trade builder  
✔ STON.fi simulation  

### **Phase 2 — Non-Custodial Automation**  
⚙️ Telegram UI  
⚙️ Execution Manager Contract  
⚙️ Backend-triggered micro-swap execution  
⚙️ Secure session lifecycle  
⚙️ User withdrawal  

### **Phase 3 — DeFi Automation Platform**  
⬜ All pulses (depth, volatility, slippage, trend, time-Decay, imbalance)  
⬜ Strategy builder (hedging, rotation)  
⬜ Backtesting UI  
⬜ Web dashboard + charts  
⬜ Multi-DEX support  
