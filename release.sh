#!/bin/bash

# Release script for nipworker monorepo
# Usage: ./release.sh <version>
# Example: ./release.sh 0.0.5

set -e  # Exit on any error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Function to print colored output
print_status() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if version is provided
if [ $# -eq 0 ]; then
    print_error "No version provided!"
    echo "Usage: $0 <version>"
    echo "Example: $0 0.0.5"
    exit 1
fi

NEW_VERSION=$1

# Validate version format (basic semver check)
if ! [[ $NEW_VERSION =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.-]+)?$ ]]; then
    print_error "Invalid version format. Please use semantic versioning (e.g., 1.0.0, 1.0.0-beta.1)"
    exit 1
fi

print_status "Starting release process for version $NEW_VERSION"

# Check if we're in the right directory
if [ ! -f "packages/nipworker/package.json" ]; then
    print_error "This script must be run from the root of the nipworker repository"
    exit 1
fi

# Check if git working directory is clean
if [ -n "$(git status --porcelain)" ]; then
    print_warning "Working directory is not clean. Uncommitted changes:"
    git status --short
    echo
    read -p "Do you want to continue? (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        print_error "Aborted by user"
        exit 1
    fi
fi

# Get current version
CURRENT_VERSION=$(node -p "require('./packages/nipworker/package.json').version")
print_status "Current version: $CURRENT_VERSION"
print_status "New version: $NEW_VERSION"

# Check if tag already exists
if git rev-parse "v$NEW_VERSION" >/dev/null 2>&1; then
    print_error "Tag v$NEW_VERSION already exists!"
    exit 1
fi

# Update nipworker package.json version
print_status "Updating @candypoets/nipworker version to $NEW_VERSION"
node -e "
const fs = require('fs');
const pkg = require('./packages/nipworker/package.json');
pkg.version = '$NEW_VERSION';
fs.writeFileSync('./packages/nipworker/package.json', JSON.stringify(pkg, null, 2) + '\n');
"

print_success "Updated nipworker version"

# Stage the changes
git add packages/nipworker/package.json

# Commit the version bump
print_status "Committing version bump"
git commit -m "Bump to v$NEW_VERSION"

# Create and push tag
print_status "Creating and pushing tag v$NEW_VERSION"
git tag "v$NEW_VERSION"

# Push changes and tag
print_status "Pushing changes and tag to origin"
git push origin main  # or your default branch
git push origin "v$NEW_VERSION"

print_success "🚀 Release v$NEW_VERSION initiated!"
print_success "✅ Version bumped and committed"
print_success "✅ Tag v$NEW_VERSION created and pushed"

echo
print_status "You can monitor the release at:"
echo "https://github.com/$(git config --get remote.origin.url | sed 's/.*github.com[:/]\([^.]*\).*/\1/')/actions"
