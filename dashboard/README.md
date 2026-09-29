# MiniLB Control Center

A small React/Vite dashboard for the MiniLB control plane and local Rust data plane.

## What it shows

- live backend health from the local MiniLB admin API
- active configuration version from the AWS control plane
- recent request telemetry from OpenSearch through the `/events` Lambda API
- recent traffic distribution by backend
- average sampled latency and 5xx rate
- enable/disable backend routing and deploy the configuration through the existing Step Functions workflow

## Security model for local development

The browser does not receive the admin token. Vite's development proxy reads `MINILB_ADMIN_TOKEN` from `.env.local` and adds `x-admin-token` to proxied control-plane requests.

This is a local-development arrangement. For a production deployment, replace the development proxy with a real authenticated frontend/backend boundary (for example Cognito/API Gateway authorizer).

## Requirements

Vite 8 currently requires Node.js 20.19+ or 22.12+. React 19.3 and the Vite React plugin 6.1 are used here.

## Setup

1. Copy `.env.example` to `.env.local`.
2. Put the existing MiniLB admin token in `MINILB_ADMIN_TOKEN`.
3. Run `npm install`.
4. Run `npm run dev`.
5. Open http://127.0.0.1:5173.

The dashboard expects the local MiniLB admin API at `http://127.0.0.1:9090` and the control-plane API configured in `MINILB_API_BASE_URL`.
