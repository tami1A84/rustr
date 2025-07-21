# Tabs
home-tab = 🏠 Home
relays-tab = 📡 Relays
profile-tab = 👤 Profile

# Login Screen
login-heading = Login or Register
secret-key-label = Secret Key (nsec or hex, for first-time setup):
secret-key-hint = Enter your nsec or hex secret key here
passphrase-label = Passphrase:
passphrase-hint = Your secure passphrase
confirm-passphrase-label = Confirm Passphrase:
confirm-passphrase-hint = Confirm your passphrase
login-button = 🔑 Login with Passphrase
register-button = ✨ Register New Key

# Home Screen
timeline-heading = Timeline
fetch-latest-button = 🔄 Fetch Latest Statuses
new-post-window-title = New Post
set-status-heading = Set Status
status-input-hint = What's on your mind?
publish-button = 🚀 Publish
cancel-button = Cancel
status-too-long = Too Long!
no-timeline-message = No timeline available. Fetch latest statuses or follow more users.

# Relays Screen
current-connection-heading = Current Connection
reconnect-button = 🔗 Re-Connect to Relays
edit-relay-lists-heading = Edit Relay Lists
nip65-relay-list-label = NIP-65 Relay List
add-relay-button = ➕ Add Relay
read-checkbox = Read
write-checkbox = Write
discover-relays-label = Discover Relays (one URL per line)
default-relays-label = Default Relays (fallback, one URL per line)
save-nip65-button = 💾 Save and Publish NIP-65 List

# Profile Screen
profile-heading = Your Profile
public-key-heading = My Public Key
copy-button = 📋 Copy
nip01-profile-heading = NIP-01 Profile Metadata
name-label = Name:
picture-url-label = Picture URL:
nip05-label = NIP-05:
lud16-label = LUD-16 (Lightning Address):
about-label = About:
other-fields-label = Other Fields (read-only for now):
save-profile-button = 💾 Save Profile
raw-json-heading = Raw NIP-01 Profile JSON
logout-button = ↩️ Logout

# Misc
fetching-profile-status = Fetching NIP-01 profile...
profile-loaded-status = Profile loaded successfully.
login-failed-status = Login failed: { $error }
profile-saved-status = Profile saved successfully!
profile-save-failed-status = Failed to save profile: { $error }
login-prompt = Existing user: Please enter your passphrase.
setup-prompt = First-time setup: Enter your secret key and set a passphrase.
invalid-nip49-format = Invalid NIP-49 format in config file.
invalid-nip49-payload = NIP-49 payload in config file is too short.
incorrect-passphrase = Incorrect passphrase.
invalid-secret-key = The provided secret key is invalid.
nip49-encryption-error = NIP-49 encryption error: { $error }
relays-not-found = No connectable relays found.
status-too-long-error = Error: Status too long! Max { $max } characters.
status-published = Status published! Event ID: { $event_id }
status-publish-failed = Failed to publish status: { $error }
event-creation-failed = Failed to create event: { $error }
nip65-publish-warning = Warning: Publishing an empty NIP-65 list.
nip65-published = NIP-65 list published! Event ID: { $event_id }
nip65-publish-failed = Failed to publish NIP-65 list: { $error }
nip01-published = NIP-01 profile published! Event ID: { $event_id }
nip01-publish-failed = Failed to publish NIP-01 profile: { $error }
profile-save-error = Error during profile save operation: { $error }
client-shutdown-failed = Failed to shutdown client on logout: { $error }
logged-out = Logged out.
please-login = Please login.
fetching-statuses = Fetching latest statuses...
no-followed-users-for-status = No followed users to fetch status from.
no-write-relays = No writeable relays found for followed users.
fetched-statuses = Fetched { $count } statuses from followed users' write relays.
fetching-metadata-for-authors = Fetching metadata for { $count } authors.
fetched-profiles = Fetched { $count } profiles.
no-new-statuses = No new statuses found for followed users.
timeline-fetch-failed = Failed to fetch timeline: { $error }
reconnecting-relays = Re-connecting to relays...
relay-connection-successful = Relay connection successful!
relay-connection-failed = Failed to connect to relays: { $error }
publishing-nip65 = Publishing NIP-65 list...
saving-profile = Saving NIP-01 profile...
publishing-status = Publishing NIP-38 status...
registering-key = Registering new key...
login-attempt = Attempting to login...
decrypting-key = Attempting to decrypt secret key...
key-decrypted = Secret key decrypted successfully. Public Key: { $pubkey }
login-complete = Login process complete!
registered-and-logged-in = Registered and logged in. Public Key: { $pubkey }
registration-failed = Failed to register new key: { $error }
passwords-do-not-match = Passphrases do not match.
