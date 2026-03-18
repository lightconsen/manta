#!/bin/bash
# Test script for agent team creation and interaction

set -e  # Exit on error

echo "=========================================="
echo "Testing Manta Agent Team Functionality"
echo "=========================================="
echo ""

# Clean up any existing test data
echo "🧹 Cleaning up existing test data..."
manta agent remove test-lead --force 2>/dev/null || true
manta agent remove test-coder --force 2>/dev/null || true
manta agent remove test-reviewer --force 2>/dev/null || true
manta team delete test-dev-team --force 2>/dev/null || true

echo ""
echo "=========================================="
echo "Step 1: Creating Agents"
echo "=========================================="

echo ""
echo "👉 Creating test-lead agent..."
manta agent create test-lead \
  --role "Team Lead" \
  --style professional \
  --prompt "You are the team lead. You coordinate work, make decisions, and delegate tasks."

echo ""
echo "👉 Creating test-coder agent..."
manta agent create test-coder \
  --role "Software Developer" \
  --style concise \
  --prompt "You are a skilled developer. You write clean, efficient code."

echo ""
echo "👉 Creating test-reviewer agent..."
manta agent create test-reviewer \
  --role "Code Reviewer" \
  --style detailed \
  --prompt "You are a thorough code reviewer. You catch bugs and suggest improvements."

echo ""
echo "✅ Agents created successfully!"
manta agent list

echo ""
echo "=========================================="
echo "Step 2: Creating Team"
echo "=========================================="

echo ""
echo "👉 Creating test-dev-team..."
manta team create test-dev-team \
  --description "Test development team for verification" \
  --type hierarchical \
  --agents "test-lead,test-coder,test-reviewer"

echo ""
echo "✅ Team created!"
manta team list

echo ""
echo "=========================================="
echo "Step 3: Setting Hierarchy"
echo "=========================================="

echo ""
echo "👉 Setting test-lead as manager..."
manta team set-hierarchy test-dev-team \
  --structure "test-lead:test-coder,test-reviewer"

echo ""
echo "✅ Hierarchy set!"

echo ""
echo "=========================================="
echo "Step 4: Setting Communication Pattern"
echo "=========================================="

echo ""
echo "👉 Setting STAR pattern..."
manta team set-communication test-dev-team \
  --pattern star \
  --shared-memory "test-canvas"

echo ""
echo "✅ Communication pattern set!"

echo ""
echo "=========================================="
echo "Step 5: Showing Team Details"
echo "=========================================="

echo ""
manta team show test-dev-team

echo ""
echo "=========================================="
echo "Step 6: Activating Team"
echo "=========================================="

echo ""
echo "👉 Activating team (registers with mesh)..."
manta team activate test-dev-team

echo ""
echo "=========================================="
echo "Step 7: Verifying Team is Active"
echo "=========================================="

echo ""
manta team list --verbose

echo ""
echo "=========================================="
echo "Step 8: Testing Agent Roles"
echo "=========================================="

echo ""
echo "👉 Setting test-coder role to 'senior-developer'..."
manta team set-role test-dev-team test-coder --role "senior-developer"

echo ""
echo "👉 Enabling delegation for test-lead..."
manta team add-member test-dev-team test-lead --role "lead" --level 0 --can-delegate

echo ""
echo "✅ Roles updated!"
manta team show test-dev-team

echo ""
echo "=========================================="
echo "Step 9: Testing Export/Import"
echo "=========================================="

echo ""
echo "👉 Exporting team to YAML..."
manta team export test-dev-team --format yaml > /tmp/test-dev-team.yaml
echo "✅ Exported to /tmp/test-dev-team.yaml"
head -30 /tmp/test-dev-team.yaml

echo ""
echo "👉 Cloning team..."
manta team clone test-dev-team test-dev-team-backup
manta team list

echo ""
echo "=========================================="
echo "Step 10: Deactivating and Cleanup"
echo "=========================================="

echo ""
echo "👉 Deactivating team..."
manta team deactivate test-dev-team

echo ""
echo "👉 Cleaning up test teams..."
manta team delete test-dev-team --force
manta team delete test-dev-team-backup --force

echo ""
echo "👉 Cleaning up test agents..."
manta agent remove test-lead --force
manta agent remove test-coder --force
manta agent remove test-reviewer --force

echo ""
echo "=========================================="
echo "✅ All Tests Completed Successfully!"
echo "=========================================="
