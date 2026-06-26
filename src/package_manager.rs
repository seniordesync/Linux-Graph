use anyhow::{Context, Result, bail};
use std::collections::{HashMap, HashSet};
use std::process::Output;
use tokio::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PackageSource {
    #[default]
    Native, // Системные пакеты (pacman, dpkg и т.д.)
    Foreign, // AUR или сторонние пакеты
    Flatpak, // Flatpak песочницы
}

#[derive(Debug, Clone)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub description: String,
    pub size_kb: f32,
    pub depends_on: Vec<String>,
    pub required_by: Vec<String>,
    pub source: PackageSource,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PackageManager {
    Pacman,  // Arch, Manjaro, CachyOS, EndeavourOS
    Dpkg,    // Debian, Ubuntu, Mint, Pop!_OS
    Rpm,     // Fedora, RedHat, CentOS, openSUSE (Zypper)
    Apk,     // Alpine
    Portage, // Gentoo
    Nix,     // NixOS
    Unknown,
}

pub fn detect_package_manager() -> PackageManager {
    if std::path::Path::new("/usr/bin/pacman").exists() {
        PackageManager::Pacman
    } else if std::path::Path::new("/usr/bin/dpkg").exists() {
        PackageManager::Dpkg
    } else if std::path::Path::new("/usr/bin/rpm").exists() {
        PackageManager::Rpm
    } else if std::path::Path::new("/sbin/apk").exists() {
        PackageManager::Apk
    } else if std::path::Path::new("/usr/bin/emerge").exists() {
        PackageManager::Portage
    } else if std::path::Path::new("/nix").exists()
        && (std::path::Path::new("/run/current-system").exists()
            || std::path::Path::new(&format!(
                "{}/.nix-profile",
                std::env::var("HOME").unwrap_or_default()
            ))
            .exists())
    {
        PackageManager::Nix
    } else {
        PackageManager::Unknown
    }
}

pub async fn get_all_packages() -> Result<HashMap<String, PackageInfo>> {
    let pm = detect_package_manager();

    // Сеньор-помидор стайл: Запускаем парсинг системных пакетов и Flatpak параллельно (Concurrent IO)!
    // Зачем ждать окончания работы pacman, чтобы потом запускать flatpak, если можно делать это одновременно?
    let (native_result, flatpak_result) = tokio::join!(
        async {
            match pm {
                PackageManager::Pacman => get_packages_pacman().await,
                PackageManager::Dpkg => get_packages_dpkg().await,
                PackageManager::Rpm => get_packages_rpm().await,
                PackageManager::Apk => get_packages_apk().await,
                PackageManager::Portage => get_packages_portage().await,
                PackageManager::Nix => get_packages_nix().await,
                PackageManager::Unknown => {
                    anyhow::bail!("Unsupported OS. No known package manager found.")
                }
            }
        },
        get_packages_flatpak()
    );

    let mut packages = native_result?;

    // Подгрузка Flatpak
    if let Ok(flatpaks) = flatpak_result {
        packages.extend(flatpaks);
    }

    finalize_package_graph(&mut packages);

    Ok(packages)
}

fn finalize_package_graph(packages: &mut HashMap<String, PackageInfo>) {
    // Оставляем только связи, которые реально резолвятся в текущем наборе пакетов.
    // Это убирает мусорные токены из парсеров и случайные shell-обрывки.
    let package_names: HashSet<String> = packages.keys().cloned().collect();
    for info in packages.values_mut() {
        info.depends_on = info
            .depends_on
            .iter()
            .filter_map(|dep| normalize_dependency_name(dep, &package_names))
            .collect();
        info.depends_on.sort();
        info.depends_on.dedup();
        info.required_by.clear();
    }

    // Автоматическое вычисление reverse dependencies (Required By).
    let mut required_by_map: HashMap<String, Vec<String>> = HashMap::with_capacity(packages.len());
    for (pkg_name, info) in packages.iter() {
        for dep in &info.depends_on {
            required_by_map
                .entry(dep.clone())
                .or_default()
                .push(pkg_name.clone());
        }
    }

    for (pkg_name, mut reqs) in required_by_map {
        reqs.sort();
        reqs.dedup();
        if let Some(info) = packages.get_mut(&pkg_name) {
            info.required_by = reqs;
        }
    }
}

fn ensure_command_success(output: &Output, command: &str) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let details = stderr.trim();
    if details.is_empty() {
        bail!("{command} exited with {}", output.status);
    }

    bail!("{command} exited with {}: {details}", output.status);
}

// --------------------------------------------------------
// 1. PACMAN (Arch Linux, CachyOS)
// --------------------------------------------------------
async fn get_foreign_pacman() -> HashSet<String> {
    let mut foreign = HashSet::new();
    if let Ok(output) = Command::new("pacman").arg("-Qm").output().await {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some(name) = line.split_whitespace().next() {
                foreign.insert(name.to_string());
            }
        }
    }
    foreign
}

async fn get_packages_pacman() -> Result<HashMap<String, PackageInfo>> {
    let foreign = get_foreign_pacman().await;

    let output = Command::new("pacman")
        .env("LC_ALL", "C")
        .arg("-Qi")
        .output()
        .await
        .context("Failed to run pacman -Qi")?;
    ensure_command_success(&output, "pacman -Qi")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut packages = HashMap::new();
    let mut current_pkg: Option<PackageInfo> = None;

    for line in stdout.lines() {
        if line.starts_with("Name") {
            if let Some(pkg) = current_pkg.take() {
                packages.insert(pkg.name.clone(), pkg);
            }
            let name = parse_value(line);
            let source = if foreign.contains(&name) {
                PackageSource::Foreign
            } else {
                PackageSource::Native
            };
            current_pkg = Some(PackageInfo {
                name,
                version: String::new(),
                description: String::new(),
                size_kb: 0.0,
                depends_on: Vec::new(),
                required_by: Vec::new(), // Вычисляется глобально
                source,
            });
        } else if let Some(ref mut pkg) = current_pkg {
            if line.starts_with("Version") {
                pkg.version = parse_value(line);
            } else if line.starts_with("Description") {
                pkg.description = parse_value(line);
            } else if line.starts_with("Depends On") {
                pkg.depends_on = parse_list(line);
            } else if line.starts_with("Installed Size") {
                pkg.size_kb = parse_size(line);
            }
        }
    }

    if let Some(pkg) = current_pkg {
        packages.insert(pkg.name.clone(), pkg);
    }
    Ok(packages)
}

// --------------------------------------------------------
// 2. DPKG (Debian, Ubuntu, Mint, Pop!_OS)
// --------------------------------------------------------
async fn get_packages_dpkg() -> Result<HashMap<String, PackageInfo>> {
    let output = Command::new("dpkg-query")
        .env("LC_ALL", "C")
        .arg("-W")
        .arg("-f=${Package}|${Version}|${binary:Summary}|${Installed-Size}|${Depends}\n")
        .output()
        .await
        .context("Failed to run dpkg-query")?;
    ensure_command_success(&output, "dpkg-query")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let packages = parse_dpkg_query_output(&stdout);

    Ok(packages)
}

fn parse_dpkg_query_output(stdout: &str) -> HashMap<String, PackageInfo> {
    let mut packages = HashMap::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(5, '|').collect();
        if parts.len() < 5 {
            continue;
        }

        let name = parts[0].to_string();
        if name.is_empty() {
            continue;
        }

        let version = parts[1].to_string();
        let description = parts[2].to_string();
        let size_kb = parse_nonnegative_f32(parts[3]);

        let depends_on = parse_dpkg_dependencies(parts[4]);

        packages.insert(
            name.clone(),
            PackageInfo {
                name,
                version,
                description,
                size_kb,
                depends_on,
                required_by: Vec::new(), // Вычисляется глобально
                source: PackageSource::Native,
            },
        );
    }
    packages
}

// --------------------------------------------------------
// 3. RPM (Fedora, RHEL, CentOS, openSUSE)
// --------------------------------------------------------
async fn get_packages_rpm() -> Result<HashMap<String, PackageInfo>> {
    let output = Command::new("rpm")
        .env("LC_ALL", "C")
        .arg("-qa")
        .arg("--qf")
        .arg("%{NAME}|%{VERSION}|%{SUMMARY}|%{SIZE}|[%{REQUIRENAME},]\n")
        .output()
        .await
        .context("Failed to run rpm -qa")?;
    ensure_command_success(&output, "rpm -qa")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut packages = HashMap::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(5, '|').collect();
        if parts.len() < 5 {
            continue;
        }

        let name = parts[0].to_string();
        if name.is_empty() {
            continue;
        }

        let version = parts[1].to_string();
        let description = parts[2].to_string();
        let size_bytes = parse_nonnegative_f32(parts[3]);

        let depends_str = parts[4];
        let mut depends_on = Vec::new();
        for dep in depends_str.split(',') {
            let dep = dep.trim();
            if dep.is_empty() {
                continue;
            }
            let dep_name = dep.split_whitespace().next().unwrap_or("").to_string();
            if dep_name.starts_with("rpmlib(")
                || dep_name.starts_with("rtld(")
                || dep_name.starts_with('/')
            {
                continue;
            }
            if !dep_name.is_empty() {
                depends_on.push(dep_name);
            }
        }

        depends_on.sort();
        depends_on.dedup();

        packages.insert(
            name.clone(),
            PackageInfo {
                name,
                version,
                description,
                size_kb: size_bytes / 1024.0,
                depends_on,
                required_by: Vec::new(),
                source: PackageSource::Native,
            },
        );
    }
    Ok(packages)
}

// --------------------------------------------------------
// 4. APK (Alpine Linux)
// --------------------------------------------------------
async fn get_packages_apk() -> Result<HashMap<String, PackageInfo>> {
    let db_path = "/lib/apk/db/installed";
    let contents = tokio::fs::read_to_string(db_path)
        .await
        .context("Failed to read apk db")?;

    let mut packages = HashMap::new();
    let mut current_pkg = PackageInfo {
        name: String::new(),
        version: String::new(),
        description: String::new(),
        size_kb: 0.0,
        depends_on: Vec::new(),
        required_by: Vec::new(),
        source: PackageSource::Native,
    };
    let mut in_pkg = false;

    for line in contents.lines() {
        if line.is_empty() {
            if in_pkg && !current_pkg.name.is_empty() {
                packages.insert(current_pkg.name.clone(), current_pkg.clone());
            }
            in_pkg = false;
            current_pkg = PackageInfo {
                name: String::new(),
                version: String::new(),
                description: String::new(),
                size_kb: 0.0,
                depends_on: Vec::new(),
                required_by: Vec::new(),
                source: PackageSource::Native,
            };
            continue;
        }

        in_pkg = true;
        if let Some(rest) = line.strip_prefix("P:") {
            current_pkg.name = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("V:") {
            current_pkg.version = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("T:") {
            current_pkg.description = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("I:") {
            current_pkg.size_kb = parse_nonnegative_f32(rest) / 1024.0;
        } else if let Some(rest) = line.strip_prefix("D:") {
            for dep in rest.split_whitespace() {
                let dep_name = dep.split(['=', '<', '>']).next().unwrap_or(dep);
                if !dep_name.starts_with('!')
                    && let Some(dep_name) = sanitize_dependency_name(dep_name)
                {
                    current_pkg.depends_on.push(dep_name);
                }
            }
        }
    }

    if in_pkg && !current_pkg.name.is_empty() {
        packages.insert(current_pkg.name.clone(), current_pkg);
    }
    Ok(packages)
}

// --------------------------------------------------------
// 5. PORTAGE (Gentoo)
// --------------------------------------------------------
async fn get_packages_portage() -> Result<HashMap<String, PackageInfo>> {
    let mut packages = HashMap::new();
    let db_path = "/var/db/pkg";
    let mut read_dir = tokio::fs::read_dir(db_path)
        .await
        .context("Failed to read /var/db/pkg")?;

    while let Some(category) = read_dir.next_entry().await? {
        if !category.file_type().await?.is_dir() {
            continue;
        }

        let mut pkg_dir = tokio::fs::read_dir(category.path()).await?;
        while let Some(pkg) = pkg_dir.next_entry().await? {
            if !pkg.file_type().await?.is_dir() {
                continue;
            }

            let name_with_version = pkg.file_name().to_string_lossy().to_string();
            let name = name_with_version
                .rsplit_once('-')
                .map(|x| x.0)
                .unwrap_or(&name_with_version)
                .to_string();

            let size_str = tokio::fs::read_to_string(pkg.path().join("SIZE"))
                .await
                .unwrap_or_default();
            let size_bytes = parse_nonnegative_f32(size_str.trim());

            let desc = tokio::fs::read_to_string(pkg.path().join("DESCRIPTION"))
                .await
                .unwrap_or_default();

            let rdepend = tokio::fs::read_to_string(pkg.path().join("RDEPEND"))
                .await
                .unwrap_or_default();
            let mut depends_on = Vec::new();
            for dep in rdepend.split_whitespace() {
                let clean = dep.replace(&['<', '>', '=', '!', '~'][..], "");
                let dep_name = clean.split(':').next().unwrap_or(&clean);
                if !dep_name.is_empty() {
                    let dep_name = dep_name.rsplit('/').next().unwrap_or(dep_name);
                    if let Some(dep_name) = sanitize_dependency_name(dep_name) {
                        depends_on.push(dep_name);
                    }
                }
            }
            depends_on.sort();
            depends_on.dedup();

            packages.insert(
                name.clone(),
                PackageInfo {
                    name,
                    version: String::new(),
                    description: desc.trim().to_string(),
                    size_kb: size_bytes / 1024.0,
                    depends_on,
                    required_by: Vec::new(),
                    source: PackageSource::Native,
                },
            );
        }
    }
    Ok(packages)
}

// --------------------------------------------------------
// 6. NIX (NixOS)
// --------------------------------------------------------
async fn get_packages_nix() -> Result<HashMap<String, PackageInfo>> {
    let mut target_path = "/run/current-system".to_string();

    // Если мы не на NixOS, а просто используем пакетный менеджер Nix в другом дистрибутиве
    if !std::path::Path::new(&target_path).exists()
        && let Ok(home) = std::env::var("HOME")
    {
        let profile_path = format!("{}/.nix-profile", home);
        if std::path::Path::new(&profile_path).exists() {
            target_path = profile_path;
        }
    }

    let output = Command::new("nix-store")
        .arg("-q")
        .arg("--graph")
        .arg(&target_path)
        .output()
        .await
        .context("Failed to run nix-store")?;
    ensure_command_success(&output, "nix-store -q --graph")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut packages = HashMap::new();

    let mut edges = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.contains("label=") {
            let parts: Vec<&str> = line.split('"').collect();
            if parts.len() >= 4 {
                let node_id = parts[1].to_string();
                let label = parts[3].to_string();
                let name = label
                    .rsplit_once('-')
                    .map(|x| x.0)
                    .unwrap_or(&label)
                    .to_string();

                packages.insert(
                    node_id,
                    PackageInfo {
                        name,
                        version: String::new(),
                        description: "Nix Store Path".to_string(),
                        size_kb: 0.0,
                        depends_on: Vec::new(),
                        required_by: Vec::new(),
                        source: PackageSource::Native,
                    },
                );
            }
        } else if line.contains("->") {
            let parts: Vec<&str> = line.split('"').collect();
            if parts.len() >= 5 {
                let from_id = parts[1].to_string();
                let to_id = parts[3].to_string();
                edges.push((from_id, to_id));
            }
        }
    }

    // Второй проход: связываем зависимости, когда все узлы 100% существуют
    for (from_id, to_id) in edges {
        let to_name = packages
            .get(&to_id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        if !to_name.is_empty()
            && let Some(from_pkg) = packages.get_mut(&from_id)
        {
            from_pkg.depends_on.push(to_name);
        }
    }

    for pkg in packages.values_mut() {
        pkg.depends_on.sort();
        pkg.depends_on.dedup();
    }

    let mut named_packages = HashMap::new();
    for (_, pkg) in packages {
        named_packages.insert(pkg.name.clone(), pkg);
    }
    Ok(named_packages)
}

// --------------------------------------------------------
// 7. FLATPAK
// --------------------------------------------------------
async fn get_packages_flatpak() -> Result<HashMap<String, PackageInfo>> {
    let mut packages = HashMap::new();

    if !std::path::Path::new("/usr/bin/flatpak").exists() {
        return Ok(packages);
    }

    let output = Command::new("/usr/bin/flatpak")
        .env("LC_ALL", "C")
        .arg("list")
        .arg("--all")
        .arg("--columns=application,version,description,size")
        .output()
        .await;

    if let Ok(output) = output {
        if !output.status.success() {
            return Ok(packages);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 4 {
                let name = parts[0].trim().to_string();
                let version = parts[1].trim().to_string();
                let description = parts[2].trim().to_string();
                let size_str = parts[3].trim();
                let size_kb = parse_flatpak_size(size_str);

                if !name.is_empty() {
                    packages.insert(
                        name.clone(),
                        PackageInfo {
                            name,
                            version,
                            description,
                            size_kb,
                            depends_on: Vec::new(),
                            required_by: Vec::new(),
                            source: PackageSource::Flatpak,
                        },
                    );
                }
            }
        }
    }

    Ok(packages)
}

fn parse_flatpak_size(s: &str) -> f32 {
    let mut num_str = String::new();
    let mut unit_str = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() || c == '.' || c == ',' {
            if c == ',' {
                num_str.push('.');
            } else {
                num_str.push(c);
            }
        } else if c.is_ascii_alphabetic() {
            unit_str.push(c);
        }
    }
    let amount = parse_nonnegative_f32(&num_str);
    if amount == 0.0 {
        return 0.0;
    }

    let unit = unit_str.to_ascii_lowercase();
    let size_kb = match unit.as_str() {
        "gb" | "gib" => amount * 1024.0 * 1024.0,
        "mb" | "mib" => amount * 1024.0,
        "kb" | "kib" => amount,
        "b" | "bytes" => amount / 1024.0,
        _ => 0.0,
    };

    if size_kb.is_finite() { size_kb } else { 0.0 }
}

fn parse_nonnegative_f32(s: &str) -> f32 {
    let normalized = s.trim().replace(',', ".");
    let amount = normalized.parse::<f32>().unwrap_or(0.0);
    if amount.is_finite() && amount >= 0.0 {
        amount
    } else {
        0.0
    }
}

// --------------------------------------------------------
// Helper Utils
// --------------------------------------------------------
fn sanitize_dependency_name(dep: &str) -> Option<String> {
    let dep = dep.trim();
    if dep.is_empty() {
        return None;
    }

    let dep = dep.split(">=").next().unwrap_or(dep);
    let dep = dep.split("<=").next().unwrap_or(dep);
    let dep = dep.split('=').next().unwrap_or(dep);
    let dep = dep.split('>').next().unwrap_or(dep);
    let dep = dep.split('<').next().unwrap_or(dep);
    let dep = dep.trim();

    if dep.is_empty()
        || dep.starts_with('-')
        || dep.starts_with('/')
        || dep.starts_with('.')
        || dep.chars().any(|c| {
            !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '+' || c == ':')
        })
    {
        return None;
    }

    Some(dep.to_string())
}

fn normalize_dependency_name(dep: &str, package_names: &HashSet<String>) -> Option<String> {
    let dep = sanitize_dependency_name(dep)?;

    if package_names.contains(&dep) {
        return Some(dep);
    }

    if let Some((base, _)) = dep.split_once(':')
        && package_names.contains(base)
    {
        return Some(base.to_string());
    }

    None
}

fn parse_value(line: &str) -> String {
    if let Some(idx) = line.find(':') {
        line[idx + 1..].trim().to_string()
    } else {
        String::new()
    }
}

fn parse_list(line: &str) -> Vec<String> {
    let val = parse_value(line);
    if val == "None" || val.is_empty() {
        return Vec::new();
    }
    val.split_whitespace()
        .filter_map(sanitize_dependency_name)
        .collect()
}

fn parse_dpkg_dependencies(depends_str: &str) -> Vec<String> {
    depends_str
        .split(',')
        .filter_map(|group| group.split('|').next())
        .filter_map(|dep| dep.split_whitespace().next())
        .filter_map(sanitize_dependency_name)
        .collect()
}

fn parse_size(line: &str) -> f32 {
    let val = parse_value(line);
    let parts: Vec<&str> = val.split_whitespace().collect();
    if parts.len() == 2 {
        let amount = parse_nonnegative_f32(parts[0]);
        if amount == 0.0 {
            return 0.0;
        }
        let size_kb = match parts[1] {
            "MiB" => amount * 1024.0,
            "GiB" => amount * 1024.0 * 1024.0,
            "KiB" => amount,
            "B" => amount / 1024.0,
            _ => 0.0,
        };

        if size_kb.is_finite() { size_kb } else { 0.0 }
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_parse_size_security() {
        assert_eq!(parse_size("Installed Size : 10.5 MiB"), 10.5 * 1024.0);
        assert_eq!(parse_size("Installed Size : invalid_data"), 0.0);
        assert_eq!(parse_size(""), 0.0);
        assert_eq!(parse_size("Installed Size : DROP TABLE packages;"), 0.0);
        assert_eq!(parse_size("Installed Size : NaN MiB"), 0.0);
        assert_eq!(parse_size("Installed Size : -42 MiB"), 0.0);
        assert_eq!(parse_size("Installed Size : 42 MB"), 0.0);
    }

    #[test]
    fn test_parse_list_security() {
        let deps = parse_list("Depends On : glibc>=2.2.5  bash<5.0  && rm -rf /");
        // '&&', '/' и '-rf' фильтруются; одиночный token 'rm' отсекается уже на стадии
        // разрешения по реальному списку пакетов.
        assert_eq!(deps.len(), 3);
    }

    #[test]
    fn test_parse_flatpak_size_security() {
        assert_eq!(parse_flatpak_size("750.5 MB"), 750.5 * 1024.0);
        assert_eq!(parse_flatpak_size("1.2 GB"), 1.2 * 1024.0 * 1024.0);
        assert_eq!(parse_flatpak_size("120 bytes"), 120.0 / 1024.0);
        assert_eq!(parse_flatpak_size("NaN GB"), 0.0);
        assert_eq!(parse_flatpak_size("DROP TABLE; MB"), 0.0);
        assert_eq!(parse_flatpak_size(""), 0.0);
        assert_eq!(
            parse_flatpak_size("99999999999999999999999999999999999.9 GB"),
            0.0
        );
    }

    #[test]
    fn test_normalize_dependency_name_only_keeps_known_packages() {
        let package_names = HashSet::from(["glibc".to_string(), "bash".to_string()]);

        assert_eq!(
            normalize_dependency_name("glibc", &package_names),
            Some("glibc".to_string())
        );
        assert_eq!(
            normalize_dependency_name("bash:any", &package_names),
            Some("bash".to_string())
        );
        assert_eq!(normalize_dependency_name("rm", &package_names), None);
        assert_eq!(normalize_dependency_name("&&", &package_names), None);
    }

    #[test]
    fn test_parse_dpkg_query_output_keeps_depends_after_alternative_separator() {
        let output = concat!(
            "demo|1.0|summary|128|libc6 (>= 2.34) | libc6.1, zlib1g:any, rm -rf /\n",
            "libc6|2.39|runtime|256|\n",
            "zlib1g|1.3|compression|64|\n",
        );

        let packages = parse_dpkg_query_output(output);
        let demo = packages.get("demo").unwrap();

        assert_eq!(demo.size_kb, 128.0);
        assert_eq!(demo.depends_on, vec!["libc6", "zlib1g:any", "rm"]);
    }

    #[test]
    fn test_finalize_package_graph_filters_deps_and_builds_reverse_deps() {
        let mut packages = HashMap::from([
            (
                "app".to_string(),
                PackageInfo {
                    name: "app".to_string(),
                    version: "1.0".to_string(),
                    description: String::new(),
                    size_kb: 1.0,
                    depends_on: vec![
                        "lib:any".to_string(),
                        "lib".to_string(),
                        "missing".to_string(),
                        "rm".to_string(),
                    ],
                    required_by: vec!["stale".to_string()],
                    source: PackageSource::Native,
                },
            ),
            (
                "lib".to_string(),
                PackageInfo {
                    name: "lib".to_string(),
                    version: "1.0".to_string(),
                    description: String::new(),
                    size_kb: 1.0,
                    depends_on: Vec::new(),
                    required_by: Vec::new(),
                    source: PackageSource::Native,
                },
            ),
        ]);

        finalize_package_graph(&mut packages);

        assert_eq!(packages["app"].depends_on, vec!["lib"]);
        assert_eq!(packages["lib"].required_by, vec!["app"]);
        assert!(packages["app"].required_by.is_empty());
    }
}
