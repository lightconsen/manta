#!/bin/bash
# Syscity Release Script
#
# Usage:
#   ./release.sh              # Auto-increment patch version (e.g. 0.1.1 -> 0.1.2)
#   ./release.sh --minor      # Increment minor version (e.g. 0.1.1 -> 0.2.0)
#   ./release.sh --major      # Increment major version (e.g. 0.1.1 -> 1.0.0)
#   ./release.sh --tag v1.2.3 # Use custom tag (skips version bump)
#
# Workflow:
#   1. Bump version in Cargo.toml files
#   2. Commit version bump
#   3. Push commit to main
#   4. Create and push tag

set -e

# Navigate to project root (in case script is run from scripts/)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Parse arguments
BUMP="patch"
CUSTOM_TAG=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --patch)
            BUMP="patch"
            shift
            ;;
        --minor)
            BUMP="minor"
            shift
            ;;
        --major)
            BUMP="major"
            shift
            ;;
        --tag)
            CUSTOM_TAG="$2"
            shift 2
            ;;
        -h|--help)
            echo "Usage: $0 [--patch|--minor|--major|--tag <tag_name>]"
            echo ""
            echo "Options:"
            echo "  --patch       Increment patch version (default)  e.g. 0.1.1 -> 0.1.2"
            echo "  --minor       Increment minor version            e.g. 0.1.1 -> 0.2.0"
            echo "  --major       Increment major version            e.g. 0.1.1 -> 1.0.0"
            echo "  --tag <tag>   Use a custom tag, skip version bump"
            echo "  -h, --help    Show this help message"
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            echo "Use '$0 --help' for usage."
            exit 1
            ;;
    esac
done

# Check if we are in a git repository
if ! git rev-parse --git-dir > /dev/null 2>&1; then
    echo -e "${RED}Error: Not in a git repository.${NC}"
    exit 1
fi

# Check if we are on main branch
CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD)
if [ "$CURRENT_BRANCH" != "main" ]; then
    echo -e "${RED}Error: You are on branch '$CURRENT_BRANCH'.${NC}"
    echo "Releases must be created from the 'main' branch."
    echo "Run: git checkout main && git pull origin main"
    exit 1
fi

# Check working tree is clean (except for Cargo.toml changes we will make)
if ! git diff-index --quiet HEAD --; then
    echo -e "${RED}Error: You have uncommitted changes.${NC}"
    git status --short
    echo "Please commit or stash them before releasing."
    exit 1
fi

# Pull latest to avoid conflicts
echo "Fetching latest from origin..."
git pull origin main

# Determine tag and version
if [ -n "$CUSTOM_TAG" ]; then
    TAG="$CUSTOM_TAG"
    echo "Using custom tag: $TAG"
else
    # Read current version from root Cargo.toml
    CURRENT_VERSION=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
    if [ -z "$CURRENT_VERSION" ]; then
        echo -e "${RED}Error: Could not read version from Cargo.toml.${NC}"
        exit 1
    fi

    # Parse version components
    MAJOR=$(echo "$CURRENT_VERSION" | cut -d. -f1)
    MINOR=$(echo "$CURRENT_VERSION" | cut -d. -f2)
    PATCH=$(echo "$CURRENT_VERSION" | cut -d. -f3)

    # Bump version
    case "$BUMP" in
        major)
            MAJOR=$((MAJOR + 1))
            MINOR=0
            PATCH=0
            ;;
        minor)
            MINOR=$((MINOR + 1))
            PATCH=0
            ;;
        patch)
            PATCH=$((PATCH + 1))
            ;;
    esac

    NEW_VERSION="${MAJOR}.${MINOR}.${PATCH}"
    TAG="v${NEW_VERSION}"
    echo "Bumping version: ${CURRENT_VERSION} -> ${NEW_VERSION}"
fi

# Validate tag format
if ! [[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+.*$ ]]; then
    echo -e "${YELLOW}Warning: Tag '$TAG' does not follow semver with 'v' prefix.${NC}"
    read -p "Continue anyway? (y/N) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

# Check if tag already exists locally
if git rev-parse "$TAG" > /dev/null 2>&1; then
    echo -e "${RED}Error: Tag '$TAG' already exists locally.${NC}"
    exit 1
fi

# Check if tag already exists on remote
git fetch --tags origin > /dev/null 2>&1
if git ls-remote --tags origin "refs/tags/$TAG" | grep -q "$TAG"; then
    echo -e "${RED}Error: Tag '$TAG' already exists on remote.${NC}"
    exit 1
fi

# If not using custom tag, bump versions in Cargo.toml files
if [ -z "$CUSTOM_TAG" ]; then
    echo "Updating version in Cargo.toml files..."

    # Update root Cargo.toml
    sed -i.bak "s/^version = \"${CURRENT_VERSION}\"/version = \"${NEW_VERSION}\"/" Cargo.toml
    rm -f Cargo.toml.bak

    # Update desktop Cargo.toml
    if [ -f "desktop/Cargo.toml" ]; then
        sed -i.bak "s/^version = \"${CURRENT_VERSION}\"/version = \"${NEW_VERSION}\"/" desktop/Cargo.toml
        rm -f desktop/Cargo.toml.bak
    fi

    # Stage version changes
    git add Cargo.toml
    if [ -f "desktop/Cargo.toml" ]; then
        git add desktop/Cargo.toml
    fi

    # Commit version bump
    echo "Committing version bump..."
    git commit -m "chore(release): bump version to ${NEW_VERSION}"
fi

# Push main branch
echo "Pushing main branch..."
git push origin main

# Create and push tag
echo "Creating tag $TAG..."
git tag "$TAG"

echo "Pushing tag to origin..."
git push origin "$TAG"

echo ""
echo -e "${GREEN}Release tag '$TAG' pushed successfully!${NC}"
echo ""
echo "GitHub Actions will now build and publish the release."
echo "Track progress at:"
echo "  https://github.com/$(git remote get-url origin | sed 's/.*github.com[\/:]//' | sed 's/\.git$//')/actions"
