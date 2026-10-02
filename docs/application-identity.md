# Application identity migration

Owner: AgentKosticka. Adopted 2 October 2026. Milestones are release gates;
a release cannot claim coexistence until all of its checks below pass.

| Release | Installed identity | Required migration work |
| --- | --- | --- |
| 0.2.0 | Legacy compatibility IDs | Preserve the installed APK's signing certificate, private device UUID, launcher and display settings. Document the finite transition. |
| 0.2.1 | Legacy compatibility IDs | Add explicit Android profile export/import, including the installation UUID and preferences. Export desktop display/layout settings. Exclude keys, permission grants and camera authorization. Ship this bridge before changing Android IDs. |
| 0.3.0 | Fork IDs by default | Android/Kotlin `io.github.agentkosticka.extraspace`; GTK, icon and resource root `io.github.agentkosticka.Extraspace`. Fork executable `extraspace-agent`, XDG directories `extraspace-agent`, and distinct ADB socket names/ports. Provide a separate opt-in compatibility flavor for existing signed installations. |
| 1.0.0 | Fork IDs only | End compatibility-flavor distribution after the export/import bridge and coexistence tests have passed. Preserve old published APKs for users migrating. |

The 0.3 installer must detect legacy installation files, offer the fork as a
separate app, and import profiles only with an explicit migration choice. It must
not overwrite or uninstall upstream. Android permission grants do not transfer
between package IDs: Android must grant USB/camera access to the new application.
Exported profiles must be bounded, schema-versioned and validated before import.
The Android UUID is retained only when the user deliberately imports that profile;
a clean parallel install generates a new UUID to avoid device-profile collisions.

Release gates for 0.3: install upstream and fork together; verify separate desktop
launchers/window grouping, configuration, tray names and ADB endpoints; verify
both Android packages independently open, connect and upgrade; import a 0.2.1
profile and restore its remembered display settings; uninstall the fork and verify
upstream still works. Run package/ID/resource checks in CI for both flavors, and
record the hardware migration test separately from the CI badge.

Stable tags and artifacts state which flavor they contain. The README, installer,
transport package/activity constants, D-Bus tests, GResource XML, icon generator,
Gradle IDs and Kotlin sources must change together at the 0.3 gate. Keep 0.2 IDs
until the bridge is available; a global search/replace alone loses Android data
and prevents existing signed app upgrades.
