# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.

# UFFS justfile orchestrator.

import 'just/shared.just'
import 'just/help.just'
import 'just/test.just'
import 'just/build.just'
import 'just/workflow.just'
import 'just/dev.just'
import 'just/legal.just'
import 'just/analysis.just'
import 'just/analysis_ci.just'
import 'just/cache.just'
import 'just/bench_ci.just'
import 'just/bench_uffs.just'
import 'just/profile_usb.just'
import 'just/profile_load.just'
import 'just/packaging.just'

# Default recipe - show available commands.
default: _default-help

