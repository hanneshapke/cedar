<!-- translated-from: PRIVACY.md 3c80f87a1240 -->
# Politique de confidentialité

_Dernière mise à jour : 31 octobre 2026_

> Ceci est une traduction. Seule la [version anglaise](https://github.com/hanneshapke/cedar/blob/main/PRIVACY.md) fait foi.

**cedar ne collecte aucune donnée. Point.**

Cela vaut pour le serveur cedar et pour chaque application cedar : iOS, Android, ordinateur et le chat web.

## Ce que cela signifie

- Nous ne collectons, ne stockons, ne vendons ni ne partageons aucune donnée personnelle.
- Il n'y a ni statistiques d'audience, ni télémétrie, ni rapports de plantage, ni publicité, ni pistage d'aucune sorte.
- Vous n'avez pas de compte chez nous, et il n'existe pas de service cloud cedar. Nous n'exploitons aucun serveur par lequel passent vos données.

## Où vont vos données

cedar est auto-hébergé. Les applications ne communiquent qu'avec le serveur cedar que **vous** exploitez et auquel vous les associez. Vos conversations, exécutions, fichiers et réglages restent sur ce serveur et sur vos propres appareils. Nous ne les voyons jamais.

Pour savoir si une nouvelle version est sortie, l'application Mac et Windows et le chat web interrogent GitHub, où sont publiées les versions de cedar, toutes les quelques heures. La requête contient la version installée et rien sur vous ni sur vos conversations ; GitHub la voit comme n'importe quelle visite d'une page publique. La page de configuration (ordinateur) ou `CHAT_UPDATE_CHECK=off` (serveur de chat) la désactive.

Uniquement lorsque vous cliquez sur *Configurer Docker pour moi* ou *Configurer Tailscale pour moi*, l'application Mac et Windows télécharge le programme d'installation de ce logiciel auprès de son éditeur (Docker Desktop depuis desktop.docker.com, Tailscale depuis pkgs.tailscale.com) et l'installe. Comme tout téléchargement, la requête ne contient rien sur vous ni sur vos conversations. `scripts/cedar-stack.sh` fait de même sous Linux (get.docker.com, tailscale.com) lorsque vous répondez oui. Docker et Tailscale sont ensuite régis par leurs propres politiques de confidentialité.

Tout ce à quoi vous choisissez de connecter cedar (par exemple votre serveur LLM, ou un outil tiers que vous installez depuis le registre de la communauté) est régi par la politique de confidentialité de ce service, et non par la nôtre.

## Mémoire

cedar peut retenir des informations sur vous d'une conversation à l'autre, comme votre nom, vos préférences ou un projet sur lequel vous travaillez, pour que vous n'ayez pas à les répéter. **Vous gardez le contrôle de votre mémoire, et elle n'est jamais partagée avec un tiers.**

- Votre mémoire est stockée uniquement dans la base de données du serveur cedar que **vous** exploitez. Nous ne la voyons jamais, et cedar ne l'envoie jamais ni à nous ni à qui que ce soit.
- Le seul endroit où elle va est le serveur LLM que vous avez configuré, dans le cadre de chaque requête, comme le reste de votre conversation. S'il s'agit d'un fournisseur de modèles hébergés plutôt que de votre propre serveur, sa politique de confidentialité s'applique à ce qu'il reçoit.
- cedar conserve les faits, préférences et instructions permanentes que vous avez exprimés, chacun avec les mots dont il est tiré, une note sur le sujet de chaque conversation, les façons de travailler qui ont bien fonctionné, et un court profil de vous. Un fait n'est conservé que s'il cite vos propres mots dans la conversation, jamais ce qu'a dit un outil, une page web, un e-mail ou un document, et un texte qui ressemble à un mot de passe, une clé ou un numéro de carte, de compte ou d'identité officiel est refusé.
- Vous pouvez tout voir, la conversation dont provient chaque souvenir, quand il a été utilisé pour la dernière fois et comment il a évolué, et modifier, épingler, archiver ou supprimer un souvenir ou l'ensemble à tout moment dans Réglages → Mémoire (chat web et ordinateur ; les applications iOS et Android affichent la liste et suppriment) ou via l'API de cedar (`/v1/memory`). La trace d'une exécution indique exactement quels souvenirs lui ont été montrés.
- Un souvenir supprimé est retiré immédiatement de la base de données, avec son historique, et n'est plus utilisé, dès votre prochain message. Un souvenir archivé n'apparaît plus dans aucun prompt et reste jusqu'à ce que vous le supprimiez.
- Vous pouvez désactiver la mémoire dans Réglages → Outils → Mémoire, et la personne qui exploite le serveur peut la désactiver complètement (`MEMORY=off`).

## Contribuer une conversation (sur demande)

cedar peut vous permettre de *donner* une conversation à un corpus partagé servant à entraîner des modèles d'agents à poids ouverts. Il s'agit d'une proposition en cours (voir `docs/design/contributions.md`) ; ce qui existe aujourd'hui est décrit ici pour que la première phrase de cette politique reste vraie à mesure que la fonction se construit.

- **Rien n'est envoyé si vous ne l'envoyez pas.** Votre serveur cedar-chat connaît un corpus, celui du projet, sauf si la personne qui l'exploite en a indiqué un autre ou a désactivé la fonction (`CHAT_CONTRIBUTIONS_URL`) ; le connaître n'envoie rien. Vous choisissez une fois, dans Réglages → Contribuer, si vous contribuez, sous quelle licence et si vous utilisez un pseudonyme aléatoire. Ensuite, chaque contribution est une conversation, choisie par vous, après une page qui montre exactement ce qui va quitter votre serveur. Il n'y a pas d'option « tout partager », pas de collecte en arrière-plan et pas de rappels.
- **Ce que contient une contribution.** Vos prompts, les réponses et appels d'outils de l'agent, le prompt système et les définitions d'outils avec lesquels il a fonctionné, la façon dont l'exécution s'est terminée et le résultat que vous avez signalé. Avant que vous ne la voyiez, cedar-chat retire le contenu et le nom des pièces jointes, votre liste de souvenirs, les identifiants d'exécution et de clé, les horodatages (en gardant le jour) et votre région (en gardant la langue), et remplace les adresses e-mail, numéros de téléphone et de compte, adresses IP, clés et noms de dossiers personnels par des substituts inventés de même forme (une adresse chez example.com, un numéro en 555, une adresse IP de documentation, un nom inventé), le même substitut pour la même valeur dans cette conversation, jamais dérivé de la valeur. La contribution indique quelles valeurs sont des substituts. Les conversations qui ont utilisé votre Gmail ou votre agenda, ou un outil de santé, ne peuvent pas être contribuées du tout.
- **Le masquage se fait au mieux.** L'analyse repère ce qui a une forme reconnaissable. Un fait énoncé avec vos propres mots n'est pas repéré ; la page de relecture sert à le vérifier, et elle vous permet de masquer davantage et de rétablir ce que l'analyse a mal identifié.
- **Aucune identité.** Une contribution ne porte aucun compte, e-mail, clé, appareil ni adresse vous appartenant. Son pseudonyme, si vous en avez choisi un, est un identifiant aléatoire généré sur votre serveur, que vous pouvez renouveler ou abandonner. L'exploitant du corpus voit la connexion comme n'importe quel serveur web et s'engage, dans sa propre politique, à ne pas conserver de journaux de requêtes.
- **Vous pouvez retirer une contribution.** Votre serveur conserve un reçu pour chaque contribution, avec un jeton que lui seul connaît, et Réglages → Contribuer vous permet d'en retirer une. Une contribution retirée est supprimée de la file d'entrée du corpus et exclue de toute publication ultérieure ; une publication déjà diffusée est un instantané immuable et ne peut pas être reprise à ceux qui l'ont téléchargée.
- **Licence.** Vous placez ce que vous avez écrit sous la licence que vous avez choisie (CC0 ou CC BY 4.0). Les réponses de l'agent sont la sortie du modèle que votre serveur a exécuté ; le corpus ne publie que les modèles dont les conditions le permettent.

## Autorisations de l'appareil

Les applications demandent quelques autorisations, utilisées uniquement sur votre appareil et uniquement pour ce qui suit :

- **Appareil photo** – pour scanner le code QR qui associe l'application à votre serveur cedar. Les images ne sont jamais stockées ni envoyées nulle part.
- **Réseau local / Internet** – pour joindre votre propre serveur cedar.
- **Notifications** (Android) – pour vous prévenir quand une exécution est terminée.

## Enfants

Comme nous ne collectons aucune donnée, nous n'en collectons pas non plus auprès des enfants.

## Modifications

Si cette politique change un jour, la nouvelle version sera publiée dans le fichier PRIVACY.md du dépôt.

## Contact

Des questions ? Ouvrez une issue sur https://github.com/hanneshapke/cedar/issues.
