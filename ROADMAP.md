# Fil — Roadmap détaillée

## Phase 1 — Fondations & Daemon local
**Objectif** : `Ghostty → fil → bash`, totalement transparent.

### Étapes

#### 1.1 Setup du monorepo
- [ ] Init git repo
- [ ] Créer le Cargo workspace (Cargo.toml racine)
- [ ] Créer la crate `fil-protocol` (lib, messages protobuf)
- [ ] Créer la crate `fil-daemon` (bin, le proxy PTY)
- [ ] Créer la crate `fil-hub` (bin, skeleton vide pour l'instant)
- [ ] Setup CI de base (cargo check, cargo test, cargo clippy)
- [ ] Ajouter les dépendances : tokio, prost, nix, tracing

#### 1.2 Définir le protocole (protobuf)
- [ ] Message `SessionCreated` (session_id, shell, cwd, created_at)
- [ ] Message `SessionDestroyed` (session_id, exit_code)
- [ ] Message `SessionData` (session_id, bytes)
- [ ] Message `SessionResize` (session_id, cols, rows)
- [ ] Message `Heartbeat` (device_id, sessions[])
- [ ] Message `DeviceRegister` (device_id, device_name, user_token)
- [ ] Générer le code Rust via prost-build

#### 1.3 Création du PTY
- [ ] Fork du process avec `nix::unistd::forkpty()`
- [ ] Exec du shell utilisateur ($SHELL ou /bin/zsh)
- [ ] Passer les variables d'environnement (TERM, PATH, HOME, etc.)
- [ ] Configurer la taille initiale du PTY (TIOCSWINSZ)
- [ ] Stocker le file descriptor maître du PTY

#### 1.4 Proxy de bytes
- [ ] Mettre stdin en raw mode (désactiver echo, buffering, etc.)
- [ ] Boucle async : lire stdin → écrire sur le PTY fd
- [ ] Boucle async : lire le PTY fd → écrire sur stdout
- [ ] Utiliser tokio::io pour l'async I/O sur les fd
- [ ] Gérer les lectures partielles et le buffering

#### 1.5 Gestion des signaux
- [ ] Intercepter SIGWINCH → propager le resize au PTY (TIOCSWINSZ)
- [ ] Intercepter SIGTERM → tuer proprement le child process
- [ ] Intercepter SIGINT → passer au PTY (ne pas terminer fil)
- [ ] Gérer SIGCHLD → détecter quand le shell meurt
- [ ] Restaurer le terminal (raw mode off) à la sortie

#### 1.6 Cycle de vie
- [ ] Détecter la fin du child process (waitpid)
- [ ] Propager le exit code du shell comme exit code de fil
- [ ] Cleanup : fermer le PTY fd, restaurer le terminal
- [ ] Gérer le cas où le terminal parent meurt (SIGHUP)

### Tests Phase 1

#### T1.1 — Shell interactif basique
- [ ] `fil` lance un shell et on peut taper des commandes
- [ ] `ls`, `cd`, `pwd`, `echo` fonctionnent normalement
- [ ] Le prompt s'affiche correctement (PS1, starship, oh-my-zsh)

#### T1.2 — Transparence complète
- [ ] Les couleurs ANSI s'affichent (tester avec `ls --color`)
- [ ] Les 256 couleurs fonctionnent (tester avec un script de palette)
- [ ] Les true colors (24-bit) fonctionnent
- [ ] Les caractères Unicode s'affichent (emoji, CJK, accents)

#### T1.3 — Séquences de contrôle
- [ ] Ctrl+C interrompt le process en cours
- [ ] Ctrl+D fait exit du shell
- [ ] Ctrl+Z suspend un process (fg pour reprendre)
- [ ] Ctrl+L clear le terminal
- [ ] Ctrl+R fait la recherche dans l'historique

#### T1.4 — Applications TUI
- [ ] `vim` / `nvim` fonctionne (insertion, navigation, modes)
- [ ] `htop` / `top` s'affiche et se met à jour
- [ ] `less` / `man` fonctionne avec le scroll
- [ ] `tmux` fonctionne à l'intérieur de fil
- [ ] `fzf` fonctionne (sélection interactive)

#### T1.5 — Resize
- [ ] Redimensionner la fenêtre Ghostty → le contenu s'adapte
- [ ] `vim` se redessine correctement après resize
- [ ] `htop` se redessine correctement après resize
- [ ] `tput cols` et `tput lines` retournent les bonnes valeurs

#### T1.6 — Features terminal avancées
- [ ] Tab completion fonctionne (bash, zsh, fish)
- [ ] OSC 52 (copier-coller séquence) fonctionne
- [ ] Les événements souris passent (vim mouse mode, htop)
- [ ] Le scroll natif de Ghostty fonctionne (scrollback)
- [ ] Les hyperlinks OSC 8 fonctionnent
- [ ] Le bracketed paste mode fonctionne

#### T1.7 — Cycle de vie
- [ ] `exit` dans le shell → fil se termine avec code 0
- [ ] Un shell qui crash → fil se termine avec le bon exit code
- [ ] Fermer Ghostty → fil et le shell meurent proprement
- [ ] Pas de process zombie après la sortie
- [ ] Le terminal est restauré proprement (pas de raw mode résiduel)

#### T1.8 — Performance
- [ ] `cat` d'un gros fichier (>10MB) → pas de lag visible
- [ ] `yes` → le débit est comparable à un shell direct
- [ ] Pas de latence perceptible sur les frappes clavier
- [ ] La mémoire reste stable (pas de leak sur une session longue)

---

## Phase 2 — Hub & Auth
**Objectif** : `fil setup` → navigateur → connecté, device enregistré.

### Étapes

#### 2.1 Hub server skeleton
- [ ] Créer le serveur HTTP avec axum
- [ ] Setup tokio runtime
- [ ] Endpoint `/health` → 200 OK
- [ ] Configuration via variables d'environnement (PORT, DATABASE_URL, etc.)
- [ ] Setup tracing/logging structuré
- [ ] Graceful shutdown (SIGTERM)

#### 2.2 Base de données
- [ ] Intégrer SQLite via rusqlite ou sqlx
- [ ] Table `users` (id, provider, provider_id, email, public_key, created_at)
- [ ] Table `devices` (id, user_id, name, device_key, last_seen, created_at)
- [ ] Table `oauth_states` (state, redirect_uri, created_at, expires_at)
- [ ] Migrations automatiques au démarrage
- [ ] Index sur user_id, provider_id

#### 2.3 OAuth — Sign in with Apple
- [ ] Endpoint `/auth/apple/start` → redirige vers Apple
- [ ] Endpoint `/auth/apple/callback` → reçoit le code
- [ ] Valider le JWT Apple (id_token)
- [ ] Extraire l'identité (sub, email)
- [ ] Créer/retrouver le user en base
- [ ] Générer un token de session (JWT signé par le hub)

#### 2.4 OAuth — Sign in with GitHub
- [ ] Endpoint `/auth/github/start` → redirige vers GitHub
- [ ] Endpoint `/auth/github/callback` → reçoit le code
- [ ] Échanger le code contre un access_token
- [ ] Appeler l'API GitHub pour récupérer le profil
- [ ] Créer/retrouver le user en base
- [ ] Générer un token de session

#### 2.5 API devices
- [ ] Middleware d'authentification (vérifier le JWT sur chaque requête)
- [ ] `POST /devices` → enregistrer un nouveau device
- [ ] `GET /devices` → lister les devices du user
- [ ] `DELETE /devices/:id` → supprimer un device
- [ ] Générer un device_token pour la connexion WebSocket

#### 2.6 Dockerfile
- [ ] Multi-stage build (builder + runtime)
- [ ] Image finale minimale (distroless ou alpine)
- [ ] Exposer le port configurable
- [ ] Volume pour la base SQLite
- [ ] docker-compose.yml avec les env vars

#### 2.7 `fil setup` (côté daemon)
- [ ] Sous-commande `fil setup`
- [ ] Détecter les terminaux installés (Ghostty, kitty, iTerm2, Alacritty, WezTerm)
- [ ] Demander confirmation pour chaque terminal détecté
- [ ] Créer un backup de la config avant modification
- [ ] Ajouter `command = fil` (ou équivalent) dans la config du terminal
- [ ] Ouvrir le navigateur pour l'OAuth (serveur HTTP local temporaire pour le callback)
- [ ] Recevoir le token, le stocker dans `~/.config/fil/config.toml`
- [ ] Enregistrer le device auprès du hub
- [ ] Afficher un résumé : ✓ Signed in, ✓ Device registered, ✓ Connected

### Tests Phase 2

#### T2.1 — Hub server
- [ ] `GET /health` retourne 200
- [ ] Le hub démarre et écoute sur le port configuré
- [ ] Le hub se termine proprement sur SIGTERM
- [ ] Les logs sont structurés (JSON)
- [ ] Le hub crée la base SQLite et les tables au premier démarrage

#### T2.2 — OAuth
- [ ] Le flow Apple complet fonctionne (start → Apple → callback → token)
- [ ] Le flow GitHub complet fonctionne
- [ ] Un même user qui se reconnecte récupère son compte existant
- [ ] Un state OAuth expiré est rejeté
- [ ] Un token invalide est rejeté par le middleware auth

#### T2.3 — API devices
- [ ] Enregistrer un device retourne un device_id + device_token
- [ ] Lister les devices retourne uniquement ceux du user authentifié
- [ ] Supprimer un device fonctionne
- [ ] Un user ne peut pas voir/supprimer les devices d'un autre user
- [ ] Un token expiré/invalide retourne 401

#### T2.4 — Docker
- [ ] `docker build` réussit
- [ ] `docker run` démarre le hub
- [ ] Le hub persiste les données entre redémarrages (volume SQLite)
- [ ] `docker-compose up` lance tout en une commande

#### T2.5 — `fil setup`
- [ ] Détecte correctement Ghostty installé
- [ ] Crée un backup de la config avant modification
- [ ] Ajoute la bonne ligne dans la config Ghostty
- [ ] Ouvre le navigateur pour l'OAuth
- [ ] Stocke le token dans `~/.config/fil/config.toml`
- [ ] Enregistre le device au hub
- [ ] Si relancé, détecte que c'est déjà configuré

---

## Phase 3 — Daemon ↔ Hub (synchro)
**Objectif** : le hub reflète en temps réel les sessions de chaque daemon.

### Étapes

#### 3.1 Connexion WebSocket
- [ ] Le daemon se connecte au hub en WebSocket à son démarrage
- [ ] Authentification via le device_token dans le handshake
- [ ] Le hub vérifie le token et associe la connexion au user/device
- [ ] TLS sur la connexion WebSocket (wss://)

#### 3.2 Events de session
- [ ] Quand le daemon crée un PTY → envoie `SessionCreated` au hub
- [ ] Quand le shell meurt → envoie `SessionDestroyed` au hub
- [ ] Le hub met à jour son registre en mémoire
- [ ] Le hub broadcast l'événement aux autres devices du même user (pour l'app iOS)

#### 3.3 Heartbeat
- [ ] Le daemon envoie un heartbeat toutes les 5s
- [ ] Le heartbeat contient la liste des sessions actives
- [ ] Le hub compare avec son état interne et réconcilie les différences
- [ ] Si le hub a une session que le daemon n'a pas → la supprimer
- [ ] Si le daemon a une session que le hub n'a pas → l'ajouter

#### 3.4 Reconnexion
- [ ] Si la connexion WebSocket tombe → reconnexion avec backoff exponentiel
- [ ] À la reconnexion → full sync (le daemon envoie toutes ses sessions)
- [ ] Le hub remplace son état pour ce device par le nouvel état
- [ ] Pendant la déconnexion → le hub marque les sessions "unreachable"

#### 3.5 Gestion des états côté hub
- [ ] Session "online" → le daemon est connecté et la session existe
- [ ] Session "unreachable" → le daemon a perdu la connexion (< 30s)
- [ ] Session "offline" → le daemon est déconnecté depuis > 30s
- [ ] Purge → après un timeout configurable, supprimer les sessions mortes
- [ ] API `GET /sessions` → retourne toutes les sessions du user avec leur état

#### 3.6 Streaming des données terminal
- [ ] Le daemon forward les bytes du PTY au hub via WebSocket
- [ ] Le hub route les bytes vers les clients connectés (app iOS)
- [ ] Les clients peuvent envoyer des bytes (input) via le hub → daemon → PTY
- [ ] Multiplexage : plusieurs sessions sur la même connexion WebSocket
- [ ] Buffering du scrollback (dernières N lignes) pour le catch-up à la connexion

### Tests Phase 3

#### T3.1 — Connexion
- [ ] Le daemon se connecte au hub au démarrage
- [ ] Un token invalide → connexion refusée
- [ ] Le hub loggue la connexion/déconnexion de chaque device

#### T3.2 — Sync des sessions
- [ ] Ouvrir un terminal → la session apparaît dans `GET /sessions`
- [ ] Fermer le terminal → la session disparaît
- [ ] Ouvrir 5 terminaux → 5 sessions visibles
- [ ] Le hub ne montre que les sessions du user authentifié

#### T3.3 — Heartbeat & Réconciliation
- [ ] Le heartbeat arrive toutes les ~5s (vérifier les logs hub)
- [ ] Tuer un shell avec `kill -9` → le heartbeat suivant nettoie la session fantôme
- [ ] Aucune session fantôme après 10s

#### T3.4 — Reconnexion
- [ ] Couper le réseau → les sessions passent en "unreachable"
- [ ] Rétablir le réseau → reconnexion automatique + full sync
- [ ] Les sessions repassent en "online"
- [ ] Aucune session perdue ou dupliquée après reconnexion

#### T3.5 — Streaming
- [ ] Taper une commande sur le Mac → les bytes arrivent au hub
- [ ] Un client connecté au hub reçoit les bytes en temps réel
- [ ] Envoyer des bytes depuis un client → ils arrivent au PTY
- [ ] Le latence est < 50ms en LAN, < 200ms en WAN
- [ ] Plusieurs sessions streamées en parallèle sans interférence

#### T3.6 — Multi-device
- [ ] 2 Macs connectés → le hub voit les sessions des deux
- [ ] Éteindre un Mac → ses sessions passent offline, l'autre Mac n'est pas affecté

---

## Phase 4 — App iOS MVP
**Objectif** : voir et contrôler ses sessions depuis l'iPhone.

### Étapes

#### 4.1 Projet Xcode
- [ ] Créer le projet SwiftUI
- [ ] Intégrer TCA (The Composable Architecture)
- [ ] Configurer les targets (app, widget extension, notification extension)
- [ ] Ajouter SwiftTerm comme dépendance (SPM)
- [ ] Configurer les signing & capabilities

#### 4.2 Auth
- [ ] Écran de login (Sign in with Apple + GitHub)
- [ ] Intégration AuthenticationServices pour Sign in with Apple
- [ ] Flow OAuth GitHub via ASWebAuthenticationSession
- [ ] Stocker le token dans le Keychain iOS
- [ ] Auto-login au lancement si token valide
- [ ] Écran de logout dans les settings

#### 4.3 Liste des machines & sessions
- [ ] Connexion WebSocket au hub (recevoir les updates en temps réel)
- [ ] Écran principal : liste des machines groupées
- [ ] Pour chaque machine : nom, statut (online/offline), sessions
- [ ] Pour chaque session : nom du shell, répertoire courant, durée, aperçu dernière ligne
- [ ] Pull to refresh
- [ ] Indicateurs visuels : 🟢 online, 🟡 unreachable, ⚫ offline
- [ ] Animation quand une session apparaît/disparaît

#### 4.4 Vue terminal
- [ ] Intégrer SwiftTerm dans une vue SwiftUI
- [ ] Connexion au stream de bytes via le hub
- [ ] Afficher le contenu du terminal (couleurs, curseur, etc.)
- [ ] Catch-up du scrollback à la connexion
- [ ] Scroll dans l'historique (geste natif iOS)

#### 4.5 Input
- [ ] Barre de touches extra au-dessus du clavier (Esc, Tab, Ctrl, ↑↓←→, ⌥)
- [ ] Touches extra personnalisables
- [ ] Envoyer les frappes clavier via hub → daemon → PTY
- [ ] Gérer les combinaisons Ctrl+C, Ctrl+D, Ctrl+Z
- [ ] Support du paste (coller depuis le clipboard iOS)

#### 4.6 Navigation & Gestes
- [ ] Swipe gauche/droite → session précédente/suivante
- [ ] Swipe depuis le haut → retour à la liste
- [ ] Pinch → zoom taille du texte
- [ ] Long press → sélection de texte dans le terminal
- [ ] Two-finger tap → coller

#### 4.7 Gestion de la connexion
- [ ] État de connexion visible (connecting, connected, disconnected)
- [ ] Reconnexion automatique en cas de perte réseau
- [ ] Gestion du passage background → foreground (reconnecter le stream)
- [ ] Indicateur de latence

#### 4.8 Settings
- [ ] Écran de réglages
- [ ] Taille de police
- [ ] Thème du terminal (dark par défaut, options)
- [ ] Configuration du hub URL (pour self-host)
- [ ] Gestion du compte (logout, supprimer le compte)

### Tests Phase 4

#### T4.1 — Auth
- [ ] Sign in with Apple fonctionne end-to-end
- [ ] Sign in with GitHub fonctionne end-to-end
- [ ] Le token est persisté → relancer l'app = auto-login
- [ ] Logout efface le token et revient à l'écran de login
- [ ] Token expiré → redemande le login

#### T4.2 — Liste des sessions
- [ ] Les machines et sessions s'affichent correctement
- [ ] Ouvrir un nouveau terminal sur le Mac → apparaît en temps réel sur l'iPhone
- [ ] Fermer un terminal → disparaît en temps réel
- [ ] Le statut online/offline est correct
- [ ] L'aperçu de la dernière ligne est à jour

#### T4.3 — Terminal
- [ ] Taper `ls` depuis l'iPhone → le résultat s'affiche
- [ ] Les couleurs ANSI s'affichent correctement
- [ ] vim fonctionne (insertion, Esc, :wq)
- [ ] Ctrl+C interrompt un process
- [ ] Le scroll dans l'historique fonctionne

#### T4.4 — Gestes
- [ ] Swipe entre sessions fonctionne
- [ ] Pinch zoom fonctionne
- [ ] Le clavier extra row fonctionne (chaque touche envoie le bon code)

#### T4.5 — Réseau
- [ ] Passer en mode avion → reconnexion quand le réseau revient
- [ ] Passer l'app en background 5 min → revenir → la session reprend
- [ ] La latence est acceptable sur 4G/5G (< 300ms)

#### T4.6 — Edge cases
- [ ] 0 machine connectée → message "Aucune machine" clair
- [ ] Session qui meurt pendant qu'on la regarde → message clair, retour à la liste
- [ ] Hub injoignable → message d'erreur clair avec retry

---

## Phase 5 — Chiffrement E2E
**Objectif** : le hub est aveugle, les données terminal sont chiffrées de bout en bout.

### Étapes

#### 5.1 Génération des clés
- [ ] Générer une paire de clés Ed25519 par device
- [ ] Stocker la clé privée dans le Keychain (macOS/iOS)
- [ ] Envoyer la clé publique au hub lors de l'enregistrement du device
- [ ] Le hub stocke uniquement les clés publiques

#### 5.2 Échange de clés
- [ ] Implémenter le Noise Protocol (XX handshake pattern)
- [ ] Le premier device d'un user devient la "racine de confiance"
- [ ] Les devices suivants échangent les clés via le hub (qui ne voit que du chiffrement)
- [ ] Générer des clés de session éphémères (forward secrecy)

#### 5.3 Chiffrement du stream
- [ ] Chiffrer les bytes terminal avant envoi au hub (ChaCha20-Poly1305)
- [ ] Le hub reçoit et route des blobs chiffrés
- [ ] Le device destinataire déchiffre les bytes
- [ ] Le protocole protobuf wrappe les payloads chiffrés

#### 5.4 Rotation des clés
- [ ] Rotation automatique des clés de session périodiquement
- [ ] Rekeying transparent sans interruption du stream
- [ ] Révocation d'un device → il ne peut plus déchiffrer

#### 5.5 Vérification
- [ ] Le hub ne peut pas lire le contenu des sessions (vérifier par inspection)
- [ ] Un device supprimé ne peut plus accéder aux sessions
- [ ] Les métadonnées (nom de session, shell) sont aussi chiffrées (optionnel)

### Tests Phase 5

#### T5.1 — Chiffrement
- [ ] Capturer le trafic WebSocket → les bytes sont illisibles
- [ ] Deux devices du même user peuvent communiquer
- [ ] Un device d'un autre user ne peut pas déchiffrer

#### T5.2 — Key management
- [ ] Ajouter un nouveau device → il reçoit les clés et peut voir les sessions
- [ ] Supprimer un device → il ne peut plus déchiffrer
- [ ] Les clés privées ne quittent jamais le device

#### T5.3 — Performance
- [ ] Le chiffrement n'ajoute pas plus de 5ms de latence
- [ ] Le débit reste acceptable (pas de bottleneck sur le chiffrement)

#### T5.4 — Robustesse
- [ ] Perte de paquets → le stream reprend après resync
- [ ] Reconnexion → nouveau handshake, nouvelles clés de session
- [ ] Pas de replay attack possible

---

## Phase 6 — Features iOS natives
**Objectif** : les features qui nous différencient de tous les concurrents.

### Étapes

#### 6.1 Push notifications
- [ ] Setup APNs (Apple Push Notification service)
- [ ] Le daemon détecte les événements notifiables :
  - Commande longue terminée (process running > 30s qui exit)
  - Prompt en attente d'input (pattern de prompt détecté)
  - Erreur (exit code ≠ 0)
- [ ] Le daemon envoie l'événement au hub
- [ ] Le hub push la notification via APNs
- [ ] L'app affiche la notification avec action "Ouvrir"
- [ ] Tap sur la notif → ouvre directement la session concernée
- [ ] Réglages : activer/désactiver par type de notification
- [ ] Réglages : seuil de durée pour "commande longue" (défaut 30s)

#### 6.2 Live Activities & Dynamic Island
- [ ] Créer l'ActivityAttributes pour une session
- [ ] Démarrer une Live Activity quand un process long est détecté
- [ ] Afficher sur le Dynamic Island : nom de la commande, durée, machine
- [ ] Mettre à jour en temps réel (durée qui s'incrémente)
- [ ] Terminer la Live Activity quand le process finit
- [ ] Tap sur le Dynamic Island → ouvre la session
- [ ] Afficher sur l'écran de verrouillage

#### 6.3 Widgets
- [ ] Widget Small (2x2) : nombre de sessions actives par machine
- [ ] Widget Medium (4x2) : liste des sessions avec statut
- [ ] Les widgets se mettent à jour via WidgetKit timeline
- [ ] Tap sur un widget → ouvre l'app sur la bonne session
- [ ] Configuration du widget : choisir quelle machine afficher

#### 6.4 Shortcuts & Siri
- [ ] Définir les AppIntents :
  - "Ouvrir la dernière session active"
  - "Lister mes machines"
  - "Nouvelle session sur [machine]"
  - "Exécuter [commande] sur [machine]"
- [ ] Intégration Siri : "Dis Siri, ouvre mon terminal"
- [ ] Paramètres de Shortcuts : machine, session, commande
- [ ] Support des automations (quand j'arrive au bureau → afficher les sessions)

#### 6.5 Apple Pencil (iPad)
- [ ] Détection de l'Apple Pencil
- [ ] Sélection de texte avec le Pencil
- [ ] Scribble → convertir écriture en texte → envoyer au terminal
- [ ] Annotations sur le terminal (marquer des lignes)

### Tests Phase 6

#### T6.1 — Notifications
- [ ] Lancer `sleep 60 && echo done` → recevoir la notif "done" après 60s
- [ ] Lancer une commande qui fail → recevoir la notif d'erreur
- [ ] Claude Code attend un input → recevoir la notif "attente d'approbation"
- [ ] Tap sur la notif → l'app s'ouvre sur la bonne session
- [ ] Désactiver les notifs → plus de notifications
- [ ] Mettre le seuil à 120s → une commande de 90s ne notifie pas

#### T6.2 — Live Activities
- [ ] Lancer `npm run build` (>30s) → Dynamic Island s'active
- [ ] Le timer s'incrémente en temps réel
- [ ] Le build finit → Dynamic Island se ferme
- [ ] Visible sur l'écran de verrouillage
- [ ] Tap → ouvre la session

#### T6.3 — Widgets
- [ ] Le widget Small affiche le bon nombre de sessions
- [ ] Le widget Medium affiche la liste correcte
- [ ] Un terminal s'ouvre → le widget se met à jour (au prochain refresh)
- [ ] Tap sur le widget → ouvre l'app

#### T6.4 — Shortcuts
- [ ] "Dis Siri, ouvre mon terminal" → l'app s'ouvre sur la dernière session
- [ ] Un Shortcut "lancer build" exécute la commande sur la bonne machine
- [ ] L'automation "arrivée au bureau" fonctionne

---

## Phase 7 — Distribution & Launch
**Objectif** : le monde découvre Fil.

### Étapes

#### 7.1 Homebrew
- [ ] Créer la formula Homebrew
- [ ] Tester `brew install fil` sur un Mac clean
- [ ] Tester `brew upgrade fil`
- [ ] Tester `brew uninstall fil`
- [ ] Publier sur homebrew-core ou un tap custom (homebrew-fil)

#### 7.2 `fil setup` (polish)
- [ ] Wizard interactif fluide et beau (couleurs, spinners)
- [ ] Détection de TOUS les terminaux supportés
- [ ] Gestion des cas d'erreur (pas de terminal détecté, hub injoignable)
- [ ] `fil setup --hub https://custom.url` pour le self-host
- [ ] Idempotent : relancer `fil setup` ne casse rien

#### 7.3 `fil uninstall`
- [ ] Restaurer les configs terminaux depuis les backups
- [ ] Supprimer `~/.config/fil/`
- [ ] Optionnel : supprimer le device du hub
- [ ] Optionnel : supprimer le compte du hub
- [ ] Message de confirmation avant chaque action destructive

#### 7.4 App Store
- [ ] Screenshots iPhone (6.7", 6.1")
- [ ] Screenshots iPad (12.9", 11")
- [ ] Description App Store (FR + EN)
- [ ] Keywords optimisés (terminal, SSH, remote, developer)
- [ ] Icône App Store (1024x1024)
- [ ] Privacy policy page sur fil.sh
- [ ] App Review : préparer un compte de démo + un Mac de test
- [ ] Soumettre pour review

#### 7.5 Docker Hub
- [ ] Publier l'image `fil/hub` sur Docker Hub
- [ ] README avec docker-compose.yml
- [ ] Tags : latest, version (v0.1.0), sha
- [ ] Documentation des env vars

#### 7.6 Landing page fil.sh
- [ ] Hero : tagline + animation du concept (Mac → iPhone)
- [ ] Section features (avec les screenshots iOS)
- [ ] Section "How it works" (3 étapes : install, setup, use)
- [ ] Section pricing (free? freemium? open-source?)
- [ ] Section self-host
- [ ] Footer avec liens (GitHub, Discord, Twitter)
- [ ] SEO de base (meta, OG, Twitter Card)

#### 7.7 Launch
- [ ] ProductHunt : préparer le post (titre, description, images, maker comment)
- [ ] Hacker News : Show HN post
- [ ] Reddit : r/commandline, r/terminal, r/selfhosted, r/ios
- [ ] Twitter/X : thread de lancement
- [ ] Discord : créer le serveur communautaire

### Tests Phase 7

#### T7.1 — Installation from scratch
- [ ] Sur un Mac neuf : `brew install fil && fil setup` → tout fonctionne
- [ ] L'app iOS depuis l'App Store → sign in → les sessions apparaissent
- [ ] Self-host : `docker-compose up` → `fil setup --hub` → fonctionne

#### T7.2 — Uninstall
- [ ] `fil uninstall` → les configs terminaux sont restaurées
- [ ] `brew uninstall fil` → tout est propre
- [ ] Le terminal redevient normal (lance bash/zsh directement)

#### T7.3 — Landing page
- [ ] Le site charge en < 2s
- [ ] Responsive mobile
- [ ] Les liens fonctionnent (App Store, GitHub, brew)
- [ ] OG tags corrects (preview quand partagé sur Twitter/Slack)
