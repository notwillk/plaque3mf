# syntax=docker/dockerfile:1

# Ubuntu 24.04 LTS provides a stable package baseline for the Rust engine and
# the native Linux/FFI integration without tracking the latest Ubuntu release.
FROM mcr.microsoft.com/devcontainers/base:ubuntu-24.04

ARG DEBIAN_FRONTEND=noninteractive

# Native toolchain:
#   - Clang/LLVM, CMake, Ninja, and pkg-config support FFI and native libraries.
#   - Fontconfig and FreeType support consistent PDF/font rendering work.
#   - PAPPL and CUPS headers support the proposed Linux Printer Application.
#   - ZIP and XML tools make Core 3MF packages easy to inspect in tests.
RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        build-essential \
        clang \
        clang-format \
        cmake \
        cups-client \
        libclang-dev \
        libcups2-dev \
        libfontconfig-dev \
        libfreetype-dev \
        libpappl-dev \
        libssl-dev \
        libxml2-utils \
        lld \
        lldb \
        ninja-build \
        pkg-config \
        unzip \
        zip \
        zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*
