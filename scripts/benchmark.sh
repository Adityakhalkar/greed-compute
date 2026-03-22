#!/bin/bash
# greed-compute benchmark — run on VPS
set -e

API_KEY="emp_0cf84135e0c4568060d6e1fadb8cedac4fbf5a7f6919148b"
BASE="http://localhost:8080"

echo "=== greed-compute Benchmark ==="
echo ""

# 1. Session creation speed
echo "--- Session Creation ---"
for i in 1 2 3; do
  START=$(date +%s%N)
  SID=$(curl -s -X POST $BASE/v1/session/create -H "X-API-Key:$API_KEY" -H "Content-Type:application/json" -d '{"ttl_seconds":60}' | python3 -c "import sys,json;print(json.load(sys.stdin)['session_id'])" 2>/dev/null)
  END=$(date +%s%N)
  MS=$(( (END - START) / 1000000 ))
  echo "  Run $i: ${MS}ms (session: ${SID:0:8}...)"
  # Clean up
  curl -s -X DELETE "$BASE/v1/session/$SID" -H "X-API-Key:$API_KEY" > /dev/null
done

echo ""

# 2. Create a session for execution tests
SID=$(curl -s -X POST $BASE/v1/session/create -H "X-API-Key:$API_KEY" -H "Content-Type:application/json" -d '{"ttl_seconds":120}' | python3 -c "import sys,json;print(json.load(sys.stdin)['session_id'])" 2>/dev/null)
echo "Test session: $SID"
echo ""

# 3. Basic Python
echo "--- Basic Python (print(1+1)) ---"
for i in 1 2 3; do
  START=$(date +%s%N)
  RESULT=$(curl -s -X POST "$BASE/v1/session/$SID/execute" -H "X-API-Key:$API_KEY" -H "Content-Type:application/json" -d '{"code":"print(1+1)"}')
  END=$(date +%s%N)
  MS=$(( (END - START) / 1000000 ))
  EXEC_MS=$(echo "$RESULT" | python3 -c "import sys,json;print(json.load(sys.stdin)['duration_ms'])" 2>/dev/null)
  echo "  Run $i: ${MS}ms total (${EXEC_MS}ms python)"
done

echo ""

# 4. NumPy operations
echo "--- NumPy (mean of 10K array) ---"
for i in 1 2 3; do
  START=$(date +%s%N)
  RESULT=$(curl -s -X POST "$BASE/v1/session/$SID/execute" -H "X-API-Key:$API_KEY" -H "Content-Type:application/json" -d '{"code":"import numpy as np\nx = np.random.randn(10000)\nprint(f\"{np.mean(x):.6f}\")"}')
  END=$(date +%s%N)
  MS=$(( (END - START) / 1000000 ))
  EXEC_MS=$(echo "$RESULT" | python3 -c "import sys,json;print(json.load(sys.stdin)['duration_ms'])" 2>/dev/null)
  echo "  Run $i: ${MS}ms total (${EXEC_MS}ms python)"
done

echo ""

# 5. Pandas operations
echo "--- Pandas (create + describe 1K rows) ---"
for i in 1 2 3; do
  START=$(date +%s%N)
  RESULT=$(curl -s -X POST "$BASE/v1/session/$SID/execute" -H "X-API-Key:$API_KEY" -H "Content-Type:application/json" -d '{"code":"import pandas as pd\nimport numpy as np\ndf = pd.DataFrame({\"a\": np.random.randn(1000), \"b\": np.random.randn(1000)})\nprint(df.describe())"}')
  END=$(date +%s%N)
  MS=$(( (END - START) / 1000000 ))
  EXEC_MS=$(echo "$RESULT" | python3 -c "import sys,json;print(json.load(sys.stdin)['duration_ms'])" 2>/dev/null)
  echo "  Run $i: ${MS}ms total (${EXEC_MS}ms python)"
done

echo ""

# 6. Matrix multiplication
echo "--- Matrix Multiply (100x100 @ 100x100) ---"
for i in 1 2 3; do
  START=$(date +%s%N)
  RESULT=$(curl -s -X POST "$BASE/v1/session/$SID/execute" -H "X-API-Key:$API_KEY" -H "Content-Type:application/json" -d '{"code":"import numpy as np\na = np.random.randn(100,100)\nb = np.random.randn(100,100)\nc = a @ b\nprint(c.shape)"}')
  END=$(date +%s%N)
  MS=$(( (END - START) / 1000000 ))
  EXEC_MS=$(echo "$RESULT" | python3 -c "import sys,json;print(json.load(sys.stdin)['duration_ms'])" 2>/dev/null)
  echo "  Run $i: ${MS}ms total (${EXEC_MS}ms python)"
done

echo ""

# 7. sklearn model training
echo "--- sklearn (LinearRegression on 1K samples) ---"
for i in 1 2 3; do
  START=$(date +%s%N)
  RESULT=$(curl -s -X POST "$BASE/v1/session/$SID/execute" -H "X-API-Key:$API_KEY" -H "Content-Type:application/json" -d '{"code":"from sklearn.linear_model import LinearRegression\nimport numpy as np\nX = np.random.randn(1000,5)\ny = X @ [1,2,3,4,5] + np.random.randn(1000)*0.1\nmodel = LinearRegression().fit(X,y)\nprint(f\"R2={model.score(X,y):.6f}\")"}')
  END=$(date +%s%N)
  MS=$(( (END - START) / 1000000 ))
  EXEC_MS=$(echo "$RESULT" | python3 -c "import sys,json;print(json.load(sys.stdin)['duration_ms'])" 2>/dev/null)
  echo "  Run $i: ${MS}ms total (${EXEC_MS}ms python)"
done

echo ""

# Cleanup
curl -s -X DELETE "$BASE/v1/session/$SID" -H "X-API-Key:$API_KEY" > /dev/null
echo "=== Done ==="
