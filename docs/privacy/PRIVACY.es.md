<!-- translated-from: PRIVACY.md 3c80f87a1240 -->
# Política de privacidad

_Última actualización: 31 de octubre de 2026_

> Esta es una traducción. La versión que rige es la [versión en inglés](https://github.com/hanneshapke/cedar/blob/main/PRIVACY.md).

**cedar no recopila ningún dato. Punto.**

Esto se aplica al servidor de cedar y a cada app de cedar: iOS, Android, escritorio y el chat web.

## Qué significa

- No recopilamos, almacenamos, vendemos ni compartimos ningún dato personal.
- No hay analíticas, ni telemetría, ni informes de errores, ni anuncios, ni rastreo de ningún tipo.
- No tienes una cuenta con nosotros, y no existe un servicio en la nube de cedar. No operamos ningún servidor por el que pasen tus datos.

## Adónde van tus datos

cedar se aloja por cuenta propia. Las apps hablan solo con el servidor de cedar que **tú** ejecutas y con el que las vinculas. Tus conversaciones, ejecuciones, archivos y configuración se quedan en ese servidor y en tus propios dispositivos. Nunca los vemos.

Para saber si hay una versión nueva, la app de Mac y Windows y el chat web le preguntan a GitHub, donde se publican las versiones de cedar, cada pocas horas. La solicitud lleva la versión instalada y nada sobre ti ni sobre tus conversaciones; GitHub la ve como cualquier visita a una página pública. La página de configuración (escritorio) o `CHAT_UPDATE_CHECK=off` (servidor de chat) la desactiva.

Solo cuando haces clic en *Configurar Docker automáticamente* o *Configurar Tailscale automáticamente*, la app de Mac y Windows descarga el instalador de ese programa desde su fabricante (Docker Desktop desde desktop.docker.com, Tailscale desde pkgs.tailscale.com) y lo instala. Como cualquier descarga, la solicitud no lleva nada sobre ti ni sobre tus conversaciones. `scripts/cedar-stack.sh` hace lo mismo en Linux (get.docker.com, tailscale.com) cuando respondes que sí. Docker y Tailscale se rigen entonces por sus propias políticas de privacidad.

Todo aquello con lo que decidas conectar cedar (por ejemplo, tu servidor de LLM o una herramienta de terceros que instales desde el registro de la comunidad) se rige por la política de privacidad de ese servicio, no por la nuestra.

## Memoria

cedar puede recordar datos sobre ti de una conversación a otra, como tu nombre, tus preferencias o un proyecto en el que trabajas, para que no tengas que repetirlos. **Tú mantienes el control de tu memoria, y nunca se comparte con terceros.**

- Tu memoria se guarda solo en la base de datos del servidor de cedar que **tú** ejecutas. Nunca la vemos, y cedar nunca nos la envía ni a nosotros ni a nadie más.
- El único lugar al que va es el servidor de LLM que configuraste, como parte de cada solicitud, igual que el resto de tu conversación. Si ese es un proveedor de modelos alojados en lugar de tu propio servidor, su política de privacidad se aplica a lo que recibe.
- cedar guarda los hechos, preferencias e instrucciones permanentes que expresaste, cada uno con las palabras tuyas de las que se tomó, una nota sobre de qué trató cada conversación, formas de trabajar que funcionaron bien y un perfil breve de ti. Un hecho solo se guarda cuando cita tus propias palabras de la conversación, nunca algo que dijo una herramienta, una página web, un correo o un documento, y se rechaza el texto que parece una contraseña, una clave o un número de tarjeta, de cuenta o de identificación oficial.
- Puedes verlo todo, la conversación de la que salió cada recuerdo, cuándo se usó por última vez y cómo cambió con el tiempo, y editar, fijar, archivar o eliminar un recuerdo o todos en cualquier momento en Configuración → Memoria (chat web y escritorio; las apps de iOS y Android muestran la lista y eliminan) o mediante la API de cedar (`/v1/memory`). La traza de una ejecución indica exactamente qué recuerdos se le mostraron.
- Un recuerdo eliminado se borra de la base de datos de inmediato, con su historial, y no se vuelve a usar, a partir de tu próximo mensaje. Uno archivado deja de aparecer en cualquier prompt y se queda hasta que lo elimines.
- Puedes desactivar la memoria en Configuración → Herramientas → Memoria, y quien ejecuta el servidor puede desactivarla por completo (`MEMORY=off`).

## Contribuir una conversación (opcional)

cedar puede permitirte *donar* una conversación a un corpus compartido que se usa para entrenar modelos de agentes de pesos abiertos. Es una propuesta en curso (consulta `docs/design/contributions.md`); lo que existe hoy se describe aquí para que la primera frase de esta política siga siendo cierta a medida que se construye.

- **Nada se envía si tú no lo envías.** Tu servidor de cedar-chat conoce un corpus, el del proyecto, salvo que quien lo ejecuta haya indicado otro o haya desactivado la función (`CHAT_CONTRIBUTIONS_URL`); conocerlo no envía nada. Eliges una vez, en Configuración → Contribuir, si quieres contribuir, una licencia y si usas un seudónimo aleatorio. Después, cada contribución es una conversación, elegida por ti, tras una página que muestra exactamente lo que saldrá de tu servidor. No hay un interruptor de «compartir todo», ni recopilación en segundo plano, ni recordatorios.
- **Qué contiene una contribución.** Tus prompts, las respuestas y llamadas a herramientas del agente, el prompt del sistema y las definiciones de herramientas con las que se ejecutó, cómo terminó la ejecución y el resultado que informaste. Antes de que la veas, cedar-chat quita el contenido y los nombres de los adjuntos, tu lista de recuerdos, los identificadores de ejecución y de clave, las marcas de tiempo (conserva el día) y tu región (conserva el idioma), y reemplaza correos, números de teléfono y de cuenta, direcciones IP, claves y nombres de carpetas personales por sustitutos inventados con la misma forma (una dirección en example.com, un número 555, una dirección IP de documentación, un nombre inventado), el mismo sustituto para el mismo valor dentro de esa conversación y nunca derivado del valor. La contribución indica qué valores son sustitutos. Las conversaciones que usaron tu Gmail o tu Calendar, o una herramienta de salud, no se pueden contribuir en absoluto.
- **La censura de datos se hace lo mejor posible.** El análisis detecta lo que tiene una forma reconocible. Un hecho dicho con tus propias palabras no se detecta; para eso está la página de revisión, que te permite ocultar más y restaurar lo que el análisis detectó por error.
- **Sin identidad.** Una contribución no lleva ninguna cuenta, correo, clave, dispositivo ni dirección tuya. Su seudónimo, si elegiste uno, es un identificador aleatorio generado en tu servidor que puedes cambiar o quitar. Quien opera el corpus ve la conexión como cualquier servidor web y se compromete en su propia política a no guardar registros de solicitudes.
- **Puedes retirarla.** Tu servidor guarda un recibo de cada contribución, con un token que solo él conoce, y Configuración → Contribuir te permite retirar una. Una contribución retirada se elimina de la entrada del corpus y queda fuera de cada publicación posterior; una publicación ya hecha es una instantánea inmutable y no se puede recuperar de quienes la descargaron.
- **Licencias.** Licencias lo que escribiste bajo la licencia que elegiste (CC0 o CC BY 4.0). Las respuestas del agente son la salida del modelo que ejecutó tu servidor; el corpus solo publica modelos cuyos términos lo permiten.

## Permisos del dispositivo

Las apps piden algunos permisos, que se usan solo en tu dispositivo y solo para lo siguiente:

- **Cámara** – para escanear el código QR que vincula la app con tu servidor de cedar. Las imágenes nunca se guardan ni se envían a ningún lado.
- **Red local / internet** – para llegar a tu propio servidor de cedar.
- **Notificaciones** (Android) – para avisarte cuando termina una ejecución.

## Menores

Como no recopilamos datos, tampoco recopilamos datos de menores.

## Cambios

Si esta política cambia alguna vez, la actualización se publicará en el archivo PRIVACY.md del repositorio.

## Contacto

¿Preguntas? Abre un issue en https://github.com/hanneshapke/cedar/issues.
