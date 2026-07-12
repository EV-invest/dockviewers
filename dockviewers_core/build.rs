const DEPRECATE_BY_VERSION: Option<&str> = Some("v1.0.0");
const DEPRECATE_FORCE: bool = true;

fn main() {
	git_version();
	log_directives();
	deprecate();
}

fn git_version() {
	println!("cargo:rerun-if-changed=.git/HEAD");
	println!("cargo:rerun-if-changed=.git/refs/");
	// Embed git commit hash (fallback to "unknown" if git unavailable, e.g., in Nix sandbox)
	let git_hash = std::process::Command::new("git")
		.args(["rev-parse", "--short", "HEAD"])
		.output()
		.ok()
		.and_then(|o| if o.status.success() { Some(o.stdout) } else { None })
		.and_then(|stdout| String::from_utf8(stdout).ok())
		.map(|s| s.trim().to_string())
		.unwrap_or_else(|| "unknown".to_string());
	println!("cargo:rustc-env=GIT_HASH={git_hash}");
}

fn log_directives() {
	// Embed log directives if .cargo/log_directives exists
	println!("cargo:rerun-if-changed=.cargo/log_directives");
	if let Ok(directives) = std::fs::read_to_string(".cargo/log_directives") {
		let directives = directives.trim();
		if !directives.is_empty() {
			println!("cargo:rustc-env=LOG_DIRECTIVES={directives}");
		}
	}
}

fn deprecate() {
	println!("cargo:rerun-if-changed=src");
	println!("cargo:rerun-if-changed=Cargo.toml");
	let pkg_version = env!("CARGO_PKG_VERSION");
	let current = parse_semver(pkg_version);
	let default_deprecate_at = DEPRECATE_BY_VERSION.map(parse_semver);

	let src_dir = std::path::Path::new("src");
	if src_dir.exists() {
		// Force mode: rewrite all since attributes to target version
		if DEPRECATE_FORCE {
			if let Some(target_version) = DEPRECATE_BY_VERSION {
				let mut force_updates = Vec::new();
				collect_force_updates(src_dir, target_version, &mut force_updates);
				for (path, line_num, old_line) in &force_updates {
					if let Err(e) = update_since_in_file(path, *line_num, old_line, target_version) {
						eprintln!("Warning: failed to update {}: {}", path, e);
					}
				}
				// In force mode, we just update files and exit - no validation
				return;
			}
		}

		let mut expired_items = Vec::new();
		let mut missing_since = Vec::new();
		find_deprecated_attrs(src_dir, current, default_deprecate_at, &mut expired_items, &mut missing_since);

		// Error if deprecated items without `since` and no default version
		if !missing_since.is_empty() && DEPRECATE_BY_VERSION.is_none() {
			eprintln!("\n\x1b[1;31mDeprecated items missing `since` attribute!\x1b[0m\n");
			for loc in &missing_since {
				eprintln!("  - {}", loc);
			}
			eprintln!("\nEither add `since = \"VERSION\"` to each #[deprecated] attribute,");
			eprintln!("or configure a default version: {{ deprecate = {{ by_version = \"X.Y.Z\"; }}; }}");
			panic!("Deprecated items must have `since` attribute or a default version configured");
		}

		if !expired_items.is_empty() {
			eprintln!("\n\x1b[1;31mDeprecated items past their removal deadline!\x1b[0m\n");
			for (loc, version) in &expired_items {
				eprintln!("  - {} (should be removed by {})", loc, version);
			}
			eprintln!("\nRemove these items before proceeding with version {}.", pkg_version);
			panic!("Deprecated items must be removed");
		}
	}
}

/// Parse a semver version string, handling optional 'v' prefix.
fn parse_semver(version: &str) -> (u32, u32, u32) {
	let version = version.strip_prefix('v').unwrap_or(version);
	let parts: Vec<&str> = version.split('.').collect();
	let major = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
	let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
	let patch = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
	(major, minor, patch)
}

fn find_deprecated_attrs(
	dir: &std::path::Path,
	current: (u32, u32, u32),
	default_deprecate_at: Option<(u32, u32, u32)>,
	expired: &mut Vec<(String, String)>,
	missing_since: &mut Vec<String>,
) {
	let Ok(entries) = std::fs::read_dir(dir) else {
		return;
	};

	for entry in entries.flatten() {
		let path = entry.path();
		if path.is_dir() {
			find_deprecated_attrs(&path, current, default_deprecate_at, expired, missing_since);
		} else if path.extension().is_some_and(|ext| ext == "rs") {
			if let Ok(content) = std::fs::read_to_string(&path) {
				// Auto-patch legacy v-prefixed since values
				patch_legacy_v_prefix(&path, &content);

				// Re-read content after potential patching
				let content = std::fs::read_to_string(&path).unwrap_or(content);

				for (line_num, line) in content.lines().enumerate() {
					let trimmed = line.trim_start();
					if trimmed.starts_with("#[deprecated") {
						let loc = format!("{}:{}", path.display(), line_num + 1);

						if let Some(since_version) = extract_since(trimmed) {
							// Has since attribute
							let deprecate_at = parse_semver(since_version);
							if current >= deprecate_at {
								expired.push((loc, since_version.to_string()));
							}
						} else {
							// No since attribute
							if let Some(default_at) = default_deprecate_at {
								if current >= default_at {
									expired.push((loc, DEPRECATE_BY_VERSION.unwrap().to_string()));
								}
							} else {
								missing_since.push(loc);
							}
						}
					} else if trimmed.starts_with("#[allow(deprecated") {
						// #[allow(deprecated)] should always be removed by default version
						if let Some(default_at) = default_deprecate_at {
							if current >= default_at {
								expired.push((format!("{}:{}", path.display(), line_num + 1), DEPRECATE_BY_VERSION.unwrap().to_string()));
							}
						}
					}
				}
			}
		}
	}
}

/// Collect all #[deprecated] items that need their since attribute updated
fn collect_force_updates(dir: &std::path::Path, target_version: &str, updates: &mut Vec<(String, usize, String)>) {
	let Ok(entries) = std::fs::read_dir(dir) else {
		return;
	};

	for entry in entries.flatten() {
		let path = entry.path();
		if path.is_dir() {
			collect_force_updates(&path, target_version, updates);
		} else if path.extension().is_some_and(|ext| ext == "rs") {
			if let Ok(content) = std::fs::read_to_string(&path) {
				for (line_num, line) in content.lines().enumerate() {
					let trimmed = line.trim_start();
					if trimmed.starts_with("#[deprecated") {
						// Check if since is missing or differs from target
						match extract_since(trimmed) {
							Some(v) if v == target_version => {} // Already correct
							_ => updates.push((path.display().to_string(), line_num, line.to_string())),
						}
					}
				}
			}
		}
	}
}

/// Patch legacy `since = "vX.Y.Z"` to `since = "X.Y.Z"` in a file
//DEPRECATE: had some libs wrongly formatted, - here due to legacy. Won't be ran in normal workflows, - so it's fine that we're modifying files in place.
fn patch_legacy_v_prefix(path: &std::path::Path, content: &str) {
	let mut needs_patch = false;
	for line in content.lines() {
		let trimmed = line.trim_start();
		if trimmed.starts_with("#[deprecated") {
			if let Some(since_version) = extract_since(trimmed) {
				if since_version.starts_with('v') {
					needs_patch = true;
					break;
				}
			}
		}
	}

	if !needs_patch {
		return;
	}

	let mut new_lines = Vec::new();
	for line in content.lines() {
		let trimmed = line.trim_start();
		if trimmed.starts_with("#[deprecated") {
			if let Some(since_version) = extract_since(trimmed) {
				if since_version.starts_with('v') {
					// Strip the 'v' prefix
					let corrected_version = &since_version[1..];
					let new_line = line
						.replace(&format!("since = \"{}\"", since_version), &format!("since = \"{}\"", corrected_version))
						.replace(&format!("since=\"{}\"", since_version), &format!("since = \"{}\"", corrected_version));
					new_lines.push(new_line);
					continue;
				}
			}
		}
		new_lines.push(line.to_string());
	}

	let mut new_content = new_lines.join("\n");
	if content.ends_with('\n') {
		new_content.push('\n');
	}

	if let Err(e) = std::fs::write(path, new_content) {
		eprintln!("Warning: failed to patch legacy v-prefix in {}: {}", path.display(), e);
	}
}

/// Extract the `since` value from a #[deprecated(since = "...")] attribute
fn extract_since(attr: &str) -> Option<&str> {
	let start = attr.find("since")? + 5;
	let rest = &attr[start..];
	let rest = rest.trim_start();
	let rest = rest.strip_prefix('=')?;
	let rest = rest.trim_start();
	let quote_char = rest.chars().next()?;
	if quote_char != '"' {
		return None;
	}
	let rest = &rest[1..];
	let end = rest.find('"')?;
	Some(&rest[..end])
}

/// Update a file to set/replace the since attribute on a deprecated line
fn update_since_in_file(path: &str, line_num: usize, old_line: &str, target_version: &str) -> Result<(), std::io::Error> {
	let content = std::fs::read_to_string(path)?;
	let lines: Vec<&str> = content.lines().collect();

	// Verify line still matches
	if lines.get(line_num) != Some(&old_line.as_ref()) {
		return Ok(()); // Line changed, skip
	}

	let new_line = update_deprecated_since(old_line, target_version);
	let mut new_lines: Vec<&str> = lines.clone();
	let new_line_ref: &str = &new_line;
	new_lines[line_num] = new_line_ref;

	let new_content = new_lines.join("\n");
	// Preserve trailing newline if original had one
	let new_content = if content.ends_with('\n') { new_content + "\n" } else { new_content };

	std::fs::write(path, new_content)
}

/// Update or add since attribute to a #[deprecated] line
fn update_deprecated_since(line: &str, version: &str) -> String {
	let trimmed = line.trim_start();
	let indent = &line[..line.len() - trimmed.len()];
	// Strip 'v' prefix from version for proper semver format
	let version = version.strip_prefix('v').unwrap_or(version);

	if let Some(since_start) = trimmed.find("since") {
		// Replace existing since value
		let before_since = &trimmed[..since_start];
		let after_since = &trimmed[since_start + 5..];
		// Find the = and the quoted value
		if let Some(eq_pos) = after_since.find('=') {
			let after_eq = &after_since[eq_pos + 1..].trim_start();
			if after_eq.starts_with('"') {
				if let Some(end_quote) = after_eq[1..].find('"') {
					let after_value = &after_eq[end_quote + 2..];
					return format!("{}{}since = \"{}\"{}", indent, before_since, version, after_value);
				}
			}
		}
	}

	// No since attribute, need to add one
	if trimmed == "#[deprecated]" {
		return format!("{}#[deprecated(since = \"{}\")]", indent, version);
	}

	// Has other attributes like note, add since
	if let Some(paren_start) = trimmed.find('(') {
		let before_paren = &trimmed[..paren_start + 1];
		let inside = &trimmed[paren_start + 1..];
		return format!("{}{}since = \"{}\", {}", indent, before_paren, version, inside);
	}

	// Fallback: return original
	line.to_string()
}
