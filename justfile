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

# Default recipe - show available commands.
default: _default-help

