#!/bin/bash
# Syscity Release Script
#
# Usage:
#   ./release.sh              # Auto-detect version from Cargo.toml and create tag
#   ./release.sh --tag v1.2.3 # Use custom tag

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
CUSTOM_TAG=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --tag)
            CUSTOM_TAG="$2"
            shift 2
            ;;
        -h|--help)
            echo "Usage: $0 [--tag <tag_name>]"
            echo ""
            echo "Options:"
            echo "  --tag <tag>   Use a custom tag instead of auto-detecting from Cargo.toml"
            echo "  -h, --help    Show this help message"
            echo ""
            echo "Examples:"
            echo "  $0              # Release with version from Cargo.toml (e.g., v0.1.0)"
            echo "  $0 --tag v1.0.0 # Release with custom tag"
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

# Check for uncommitted changes
if ! git diff-index --quiet HEAD --; then
    echo -e "${YELLOW}Warning: You have uncommitted changes.${NC}"
    git status --short
    read -p "Continue anyway? (y/N) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

# Determine tag name
if [ -n "$CUSTOM_TAG" ]; then
    TAG="$CUSTOM_TAG"
    echo "Using custom tag: $TAG"
else
    # Read version from Cargo.toml
    VERSION=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
    if [ -z "$VERSION" ]; then
        echo -e "${RED}Error: Could not read version from Cargo.toml.${NC}"
        exit 1
    fi
    TAG="v$VERSION"
    echo "Detected version from Cargo.toml: $VERSION"
    echo "Tag will be: $TAG"
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

# Confirm release
if [ -n "$CUSTOM_TAG" ]; then
    echo -e "${GREEN}Ready to create release with tag: $TAG${NC}"
else
    echo -e "${GREEN}Ready to create release with version $TAG from Cargo.toml${NC}"
fi
read -p "Push tag to trigger GitHub Actions release? (y/N) " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "Aborted."
    exit 0
fi

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
