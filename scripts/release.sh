#!/bin/bash
# Manta Release Script
# Usage: ./scripts/release.sh <version>
# Example: ./scripts/release.sh v0.2.0

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

VERSION=$1

if [ -z "$VERSION" ]; then
    echo -e "${RED}Error: Version required${NC}"
    echo "Usage: $0 <version>"
    echo "Example: $0 v0.2.0"
    exit 1
fi

# Validate version format
if [[ ! $VERSION =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo -e "${RED}Error: Invalid version format. Use vX.Y.Z${NC}"
    exit 1
fi

echo -e "${BLUE}🚀 Starting release process for ${VERSION}${NC}"

# Check if we're in a git repo
if [ ! -d .git ]; then
    echo -e "${RED}Error: Not a git repository${NC}"
    exit 1
fi

# Check for uncommitted changes
if ! git diff-index --quiet HEAD --; then
    echo -e "${RED}Error: Uncommitted changes detected${NC}"
    echo "Please commit or stash changes before releasing"
    exit 1
fi

# Run tests
echo -e "\n${YELLOW}📋 Running tests...${NC}"
cargo test --all-features
if [ $? -ne 0 ]; then
    echo -e "${RED}❌ Tests failed${NC}"
    exit 1
fi
echo -e "${GREEN}✅ Tests passed${NC}"

# Check formatting
echo -e "\n${YELLOW}🎨 Checking formatting...${NC}"
cargo fmt -- --check
if [ $? -ne 0 ]; then
    echo -e "${RED}❌ Formatting check failed${NC}"
    echo "Run 'cargo fmt' to fix"
    exit 1
fi
echo -e "${GREEN}✅ Formatting OK${NC}"

# Run clippy
echo -e "\n${YELLOW}🔍 Running clippy...${NC}"
cargo clippy --all-features -- -D warnings
if [ $? -ne 0 ]; then
    echo -e "${RED}❌ Clippy warnings found${NC}"
    exit 1
fi
echo -e "${GREEN}✅ Clippy OK${NC}"

# Build release binaries
echo -e "\n${YELLOW}🔨 Building release binaries...${NC}"
cargo build --release --all-features
echo -e "${GREEN}✅ Build successful${NC}"

# Run security audit
echo -e "\n${YELLOW}🔒 Running security audit...${NC}"
if command -v cargo-audit &> /dev/null; then
    cargo audit
    echo -e "${GREEN}✅ Security audit passed${NC}"
else
    echo -e "${YELLOW}⚠️  cargo-audit not installed, skipping${NC}"
fi

# Update version in Cargo.toml
echo -e "\n${YELLOW}📝 Updating version in Cargo.toml...${NC}"
sed -i.bak "s/^version = \"[0-9]\+\.[0-9]\+\.[0-9]\+\"$/version = \"${VERSION#v}\"/" Cargo.toml
rm Cargo.toml.bak
echo -e "${GREEN}✅ Updated Cargo.toml${NC}"

# Update CHANGELOG.md
echo -e "\n${YELLOW}📝 Updating CHANGELOG.md...${NC}"
DATE=$(date +%Y-%m-%d)
CHANGELOG_ENTRY="## [${VERSION#v}] - ${DATE}\n\n### Added\n- Release ${VERSION}\n\n"

if [ -f CHANGELOG.md ]; then
    # Insert after the header
    sed -i.bak "/^# Changelog/a\\
\\
${CHANGELOG_ENTRY}" CHANGELOG.md
    rm CHANGELOG.md.bak
else
    echo -e "# Changelog\n\nAll notable changes to this project will be documented in this file.\n\n${CHANGELOG_ENTRY}" > CHANGELOG.md
fi
echo -e "${GREEN}✅ Updated CHANGELOG.md${NC}"

# Commit version bump
echo -e "\n${YELLOW}💾 Committing version bump...${NC}"
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "chore: bump version to ${VERSION}"
echo -e "${GREEN}✅ Committed version bump${NC}"

# Create git tag
echo -e "\n${YELLOW}🏷️  Creating git tag...${NC}"
git tag -a "$VERSION" -m "Release $VERSION"
echo -e "${GREEN}✅ Created tag ${VERSION}${NC}"

echo -e "\n${GREEN}✨ Release ${VERSION} prepared!${NC}"
echo
echo -e "Next steps:"
echo -e "  1. Review the changes: ${BLUE}git show ${VERSION}${NC}"
echo -e "  2. Push to remote: ${BLUE}git push origin main --tags${NC}"
echo -e "  3. GitHub Actions will build and publish the release"
echo
echo -e "Or to undo:"
echo -e "  ${BLUE}git tag -d ${VERSION}${NC}"
echo -e "  ${BLUE}git reset --soft HEAD~1${NC}"
