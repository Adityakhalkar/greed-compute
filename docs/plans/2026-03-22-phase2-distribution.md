# Phase 2: Distribution Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Deploy greed-compute to production VPS, build OpenClaw skill integration, and create a landing page.

**Architecture:** greed-compute binary runs on the same DigitalOcean VPS (165.232.179.198, 2GB RAM) as OpenClaw. Reverse proxy via nginx. OpenClaw skill wraps the REST API so the agent can execute Python code as a tool.

**Tech Stack:** Rust (cross-compiled for Linux), systemd, nginx, OpenClaw skills

---

### Task 1: Cross-compile for Linux

Build greed-compute for the VPS target (Ubuntu x64).

**Steps:**
1. Add Linux target: `rustup target add x86_64-unknown-linux-gnu`
2. Cross-compile: `cargo build --release --target x86_64-unknown-linux-gnu`
3. If cross-compile fails (likely — needs Linux linker), use `cargo build --release` on the VPS directly
4. Alternative: build on VPS via git clone + cargo build

### Task 2: Deploy to VPS

**Steps:**
1. SSH into VPS, install Rust if not present
2. Clone greed-compute repo
3. Install Python3 + ML libraries (numpy, pandas, sklearn, matplotlib, scipy)
4. Build greed-compute on VPS
5. Create systemd service for auto-start
6. Configure nginx reverse proxy (greed-compute.deep-ml.com or api subdomain)
7. Test endpoints via public URL

### Task 3: OpenClaw Skill

Create a skill that wraps greed-compute API so Employee #001 can execute Python code.

**Skill file:** A SKILL.md that teaches the agent to use web.fetch to call greed-compute endpoints.

### Task 4: Landing Page

Simple page explaining what greed-compute is, how to use it, pricing.

---

Total: 4 tasks
